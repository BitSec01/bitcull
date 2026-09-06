//! Turning measurements into a verdict.
//!
//! Two ideas drive the design.
//!
//! First, sharpness is only meaningful *relative to the shoot*. A 400 mm lens
//! wide open at dusk produces different absolute Laplacian numbers than a 70 mm
//! at f/8 in daylight, so a fixed threshold would either reject a whole evening
//! match or accept everything from a bright one. We therefore normalise against
//! the distribution of the set being culled.
//!
//! Second, every deduction is recorded as a `Reason` with its point value. The
//! score is not allowed to be a black box — if the app rejects a frame, the UI
//! can say exactly why and by how much, and the user can disagree.

use crate::model::{Analysis, EyeState, Pick, Reason, ScoreSettings, Severity, Verdict};

/// Set-wide statistics used to sanity-check the absolute sharpness scale.
///
/// An earlier version normalised each frame against the set's 10th–90th
/// percentile. That was wrong: in a shoot where every frame is sharp, it
/// stretched trivial differences across the whole range and labelled
/// perfectly good photographs "badly out of focus". Relative comparison is
/// only meaningful *inside a burst*, and that already happens when group
/// members are ranked by score.
///
/// So the absolute scale decides whether a frame is soft, and these stats are
/// used only for a rescue: if an entire shoot sits below the floor — which
/// means the calibration does not suit this camera, lens or light — the scale
/// is shifted so the set is judged on its own terms instead of everything
/// being rejected.
pub struct Norms {
    /// Additive shift applied to absolute sharpness. Zero in the normal case.
    pub shift: f32,
    /// Every frame's subject sharpness, sorted, for percentile ranking.
    sorted: Vec<f32>,
    /// True when this set should be judged as scenery rather than as frames
    /// that are supposed to contain a subject.
    pub scenery: bool,
}

impl Default for Norms {
    fn default() -> Self {
        Self {
            shift: 0.0,
            sorted: Vec::new(),
            scenery: false,
        }
    }
}

impl Norms {
    /// Where `v` sits in the set, 0 (softest) .. 1 (sharpest).
    ///
    /// Returns 0.5 — a neutral answer — when there is not enough of a sample
    /// or not enough spread for a rank to mean anything.
    pub fn rank(&self, v: f32) -> f32 {
        let n = self.sorted.len();
        if n < 12 {
            return 0.5;
        }
        let lo = self.sorted[0];
        let hi = self.sorted[n - 1];
        if hi - lo < 1e-4 {
            return 0.5;
        }
        let idx = self
            .sorted
            .partition_point(|x| *x < v);
        idx as f32 / (n - 1) as f32
    }
}

/// Decide whether a set is scenery: mostly frames with nothing the detector
/// recognises.
///
/// This is a property of the *shoot*, not of individual frames. Deciding per
/// frame would be wrong in the other direction — in a football set, a frame
/// where you missed the players entirely is a failure, not a landscape.
pub fn is_scenery(items: &[Analysis], settings: &ScoreSettings) -> bool {
    match settings.subject_policy {
        crate::model::SubjectPolicy::Scenery => return true,
        crate::model::SubjectPolicy::Require => return false,
        crate::model::SubjectPolicy::Auto => {}
    }
    let usable: Vec<&Analysis> = items.iter().filter(|a| a.error.is_none()).collect();
    if usable.is_empty() {
        return false;
    }
    let with = usable
        .iter()
        .filter(|a| a.subject_stats.count > 0 && a.subject_stats.main_area > 0.0)
        .count();
    // Under a fifth carrying a subject means the detector is not the right
    // tool for this material.
    (with as f32 / usable.len() as f32) < 0.20
}

/// Build normalisation stats from analysed photos.
pub fn compute_norms(items: &[Analysis], settings: &ScoreSettings) -> Norms {
    let scenery = is_scenery(items, settings);

    let mut vals: Vec<f32> = items
        .iter()
        .filter(|a| a.error.is_none())
        .map(|a| if scenery { scenery_sharpness(a) } else { subject_sharpness(a) })
        .filter(|v| *v > 0.0)
        .collect();

    if vals.len() < 12 {
        return Norms {
            scenery,
            ..Norms::default()
        };
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let p90 = vals[((vals.len() as f32 * 0.90) as usize).min(vals.len() - 1)];

    // If even the best decile of the shoot reads as soft, the absolute scale
    // is not describing this material. Lift it so the top of the set lands
    // just above the floor, and let relative quality do the rest.
    let target = settings.sharpness_floor + 0.28;
    let shift = if p90 < target {
        (target - p90).min(0.45)
    } else {
        0.0
    };

    Norms {
        shift,
        sorted: vals,
        scenery,
    }
}

/// For scenery, sharpness means the whole frame, not one region of it.
fn scenery_sharpness(a: &Analysis) -> f32 {
    a.metrics.sharpness_peak
}

/// The sharpness figure that actually matters.
///
/// "Is this photo sharp?" is really "is the *subject* sharp?", so measure the
/// detected subject and nothing else. Frame-wide sharpness answers a different
/// question and answers it misleadingly: a wide record shot full of in-focus
/// grass reads sharper than a tight action frame with a thrown-out background.
///
/// The fallbacks only apply when there is no subject to measure.
fn subject_sharpness(a: &Analysis) -> f32 {
    // 1. The subject itself, when the detector found one.
    if a.subject_stats.count > 0 && a.subject_stats.main_area > 0.0 {
        return a.subject_stats.main_sharpness;
    }

    // 2. Failing that, the largest face.
    let face_best = a
        .faces
        .iter()
        // Ignore specks; a 20 px face carries no reliable sharpness signal.
        .filter(|f| f.area > 0.0008)
        .map(|f| f.sharpness)
        .fold(f32::NAN, f32::max);
    if !face_best.is_nan() {
        return face_best * 0.5 + a.metrics.sharpness_peak * 0.5;
    }

    // 3. Nothing identifiable — all we can do is judge the frame.
    a.metrics.sharpness_peak
}

fn normalise(v: f32, n: &Norms) -> f32 {
    (v + n.shift).clamp(0.0, 1.0)
}

/// Score one photo and populate its reasons. Pure and cheap — re-scoring the
/// whole set when a slider moves costs nothing because no pixels are touched.
pub fn score_photo(a: &mut Analysis, norms: &Norms, s: &ScoreSettings) {
    a.reasons.clear();

    if a.error.is_some() {
        a.score = 0.0;
        a.verdict = Verdict::Reject;
        a.reasons.push(Reason {
            code: "error".into(),
            label: "Could not be analysed".into(),
            severity: Severity::Bad,
            delta: 0.0,
        });
        return;
    }

    let sharp_raw = if norms.scenery {
        scenery_sharpness(a)
    } else {
        subject_sharpness(a)
    };
    // Absolute, calibrated. This is the only thing allowed to call a frame soft.
    let sharp = normalise(sharp_raw, norms);
    // Where the frame sits within this shoot. Used for ranking only.
    let sharp_rank = norms.rank(sharp_raw);

    // --- Sharpness -------------------------------------------------------
    // Two jobs, deliberately separated.
    //
    // Labelling is absolute: a frame is only called soft if it really is soft,
    // so a technically clean shoot produces no false accusations.
    //
    // Ranking is relative: on a shoot where every frame is competently focused
    // — which is the normal case for a good photographer with modern autofocus
    // — absolute sharpness is nearly constant, and a score built on it alone
    // collapses every photo into the top decile and sorts nothing. The rank
    // term restores the spread the grid needs to be orderable.
    let abs_component = if sharp >= s.sharpness_floor {
        let t = (sharp - s.sharpness_floor) / (1.0 - s.sharpness_floor).max(0.001);
        60.0 + 40.0 * t.powf(0.7)
    } else {
        60.0 * (sharp / s.sharpness_floor.max(0.001)).powf(1.4)
    };
    let rank_component = 45.0 + 55.0 * sharp_rank;
    let sharp_component = 0.4 * abs_component + 0.6 * rank_component;

    if sharp < s.sharpness_floor * 0.5 {
        a.reasons.push(Reason {
            code: "very-soft".into(),
            label: "Subject badly out of focus".into(),
            severity: Severity::Bad,
            delta: -(60.0 - sharp_component).max(0.0),
        });
    } else if sharp < s.sharpness_floor {
        a.reasons.push(Reason {
            code: "soft".into(),
            label: "Subject is soft".into(),
            severity: Severity::Warn,
            delta: -(60.0 - sharp_component).max(0.0),
        });
    } else if sharp > 0.82 {
        a.reasons.push(Reason {
            code: "sharp".into(),
            label: "Subject is tack sharp".into(),
            severity: Severity::Good,
            delta: sharp_component - 60.0,
        });
    }

    // --- Motion blur -----------------------------------------------------
    // Directional smear only counts against a frame that is also soft.
    // A sharp photo with strong lines (goalposts, pitch markings) is anisotropic
    // too, and must not be punished for it.
    let mut motion_penalty = 0.0;
    if a.metrics.motion_blur > 0.55 && sharp < 0.6 {
        motion_penalty = 14.0 * ((a.metrics.motion_blur - 0.55) / 0.45).clamp(0.0, 1.0);
        // A fast shutter rules out camera shake, so the smear is subject motion,
        // which is often wanted in sports. Halve the penalty.
        if a.meta.shutter_secs.map_or(false, |t| t <= 1.0 / 500.0) {
            motion_penalty *= 0.5;
        }
        if motion_penalty > 1.0 {
            a.reasons.push(Reason {
                code: "motion-blur".into(),
                label: format!("Motion blur (~{}°)", a.metrics.motion_angle.round()),
                severity: Severity::Warn,
                delta: -motion_penalty,
            });
        }
    }

    // --- Exposure --------------------------------------------------------
    let e = &a.metrics.exposure;
    let mut exposure_component = 100.0f32;

    if e.clipped_highlights > 0.02 {
        let p = (35.0 * ((e.clipped_highlights - 0.02) / 0.10).clamp(0.0, 1.0)).min(35.0);
        exposure_component -= p;
        if p > 3.0 {
            a.reasons.push(Reason {
                code: "blown".into(),
                label: format!("Blown highlights ({:.0}%)", e.clipped_highlights * 100.0),
                severity: if p > 18.0 { Severity::Bad } else { Severity::Warn },
                delta: -p * s.w_exposure,
            });
        }
    }
    if e.clipped_shadows > 0.12 {
        let p = (25.0 * ((e.clipped_shadows - 0.12) / 0.25).clamp(0.0, 1.0)).min(25.0);
        exposure_component -= p;
        if p > 3.0 {
            a.reasons.push(Reason {
                code: "crushed".into(),
                label: format!("Crushed shadows ({:.0}%)", e.clipped_shadows * 100.0),
                severity: Severity::Warn,
                delta: -p * s.w_exposure,
            });
        }
    }
    // A sunlit pitch legitimately sits bright, so this triggers well clear of
    // normal daylight metering rather than flagging a third of the shoot.
    if e.bias.abs() > 0.6 {
        let p = 20.0 * ((e.bias.abs() - 0.6) / 0.4).clamp(0.0, 1.0);
        exposure_component -= p;
        a.reasons.push(Reason {
            code: if e.bias > 0.0 { "over" } else { "under" }.into(),
            label: if e.bias > 0.0 {
                "Overexposed".into()
            } else {
                "Underexposed".to_string()
            },
            severity: Severity::Warn,
            delta: -p * s.w_exposure,
        });
    }
    // Flat, low-contrast frames usually mean haze, a dirty lens, or a missed
    // moment. Mild signal, mild penalty.
    if e.stddev < 0.10 {
        let p = 10.0 * ((0.10 - e.stddev) / 0.10).clamp(0.0, 1.0);
        exposure_component -= p;
        a.reasons.push(Reason {
            code: "flat".into(),
            label: "Very low contrast".into(),
            severity: Severity::Warn,
            delta: -p * s.w_exposure,
        });
    }
    let exposure_component = exposure_component.clamp(0.0, 100.0);

    // --- Faces and eyes --------------------------------------------------
    let mut face_component = 100.0f32;
    let mut disqualified = false;

    // Only faces big enough to judge get a say.
    let significant: Vec<_> = a.faces.iter().filter(|f| f.area > 0.0008).collect();

    if significant.is_empty() {
        // No usable face is genuinely not a fault — plenty of great sports
        // frames are boots, hands, or the ball, and at 400 mm most faces in
        // frame are crowd. This must stay at 100: under the deduction model
        // anything less is a flat penalty on the majority of a shoot, which is
        // exactly the bug that capped every score at 81.
        face_component = 100.0;
    } else {
        let closed: Vec<_> = significant
            .iter()
            .filter(|f| f.eye_state == EyeState::Closed)
            .collect();
        let squint: Vec<_> = significant
            .iter()
            .filter(|f| f.eye_state == EyeState::Squint)
            .collect();

        if !closed.is_empty() {
            // Weight by how prominent the affected face is: a closed-eyed
            // subject filling the frame matters far more than someone in the
            // background.
            let worst = closed
                .iter()
                .map(|f| f.area)
                .fold(0.0f32, f32::max);
            let prominence = (worst / significant.iter().map(|f| f.area).fold(0.0f32, f32::max))
                .clamp(0.0, 1.0);
            let p = (30.0 + 30.0 * prominence).min(60.0);
            face_component -= p;
            if s.strict_eyes && prominence > 0.6 {
                disqualified = true;
            }
            a.reasons.push(Reason {
                code: "eyes-closed".into(),
                label: if closed.len() == 1 {
                    "Eyes closed".into()
                } else {
                    format!("{} subjects with eyes closed", closed.len())
                },
                severity: Severity::Bad,
                delta: -p * s.w_faces,
            });
        } else if !squint.is_empty() {
            let p = 12.0;
            face_component -= p;
            a.reasons.push(Reason {
                code: "squint".into(),
                label: "Eyes partly closed".into(),
                severity: Severity::Warn,
                delta: -p * s.w_faces,
            });
        }

        // Is the face sharp?
        //
        // Not against an absolute threshold — skin is smooth, so a focused
        // face scores lower than defocused grass and an absolute test convicts
        // every portrait in the shoot. Compare the face against the sharpest
        // content in its *own* frame instead. That asks the question that
        // actually matters: did focus land on the subject, or on the turf
        // behind them? Both numbers come from the same photograph, so the
        // scene, lens and light all cancel out.
        if let Some(main) = significant
            .iter()
            .max_by(|a, b| a.area.partial_cmp(&b.area).unwrap_or(std::cmp::Ordering::Equal))
        {
            let deficit = (a.metrics.sharpness_peak - main.sharpness).max(0.0);
            if deficit > 0.28 {
                let p = (30.0 * ((deficit - 0.28) / 0.32).clamp(0.0, 1.0)).min(30.0);
                face_component -= p;
                a.reasons.push(Reason {
                    code: "soft-face".into(),
                    label: "Focus missed the face".into(),
                    severity: if p > 20.0 { Severity::Bad } else { Severity::Warn },
                    delta: -p * s.w_faces,
                });
            } else if deficit < 0.10 && main.area > 0.006 {
                a.reasons.push(Reason {
                    code: "sharp-face".into(),
                    label: "Focus on the face".into(),
                    severity: Severity::Good,
                    delta: 0.0,
                });
            }
        }
    }
    let face_component = face_component.clamp(0.0, 100.0);

    // --- Composition (weak signals) -------------------------------------
    let mut comp = 100.0f32;
    // Almost nothing in focus anywhere usually means a genuinely failed frame.
    if a.metrics.sharpness_peak < 0.12 {
        comp -= 40.0;
    }
    if a.metrics.noise > 0.75 {
        let p = 20.0 * ((a.metrics.noise - 0.75) / 0.25).clamp(0.0, 1.0);
        comp -= p;
        a.reasons.push(Reason {
            code: "noisy".into(),
            label: "Heavy noise".into(),
            severity: Severity::Warn,
            delta: -p * s.w_composition,
        });
    }
    let comp = comp.clamp(0.0, 100.0);

    // --- Subject ---------------------------------------------------------
    //
    // The strongest predictor of whether a sports frame is worth keeping is
    // simply how much of the frame the subject occupies, and whether focus
    // landed on them. Frame-wide sharpness is not just a weak substitute for
    // this — it is actively misleading, because a wide record shot full of
    // in-focus grass scores higher on it than a tight action frame.
    let st = &a.subject_stats;
    let has_subject = st.count > 0 && st.main_area > 0.0;
    // Extra deductions raised while examining the subject or the scene.
    let mut comp_extra = 0.0f32;

    let subject_component = if norms.scenery {
        // Scenery: there is no subject to find, so judge the frame itself.
        //
        // A landscape wants detail corner to corner rather than one sharp
        // region, real tonal range rather than haze, and a level horizon. The
        // subject-shaped scoring collapsed every one of these frames to the
        // same ~44 because "no recognisable subject" fired on all of them.
        let coverage = a.metrics.detail_coverage;
        let tonal = (a.metrics.exposure.stddev / 0.22).clamp(0.0, 1.0);
        let depth = ((coverage - 0.25) / 0.55).clamp(0.0, 1.0);

        45.0 + 55.0 * (0.50 * sharp_rank + 0.30 * depth + 0.20 * tonal)
    } else if has_subject {
        let prominence = crate::subjects::prominence(st.main_area);

        // Did focus land on the subject, or on the turf behind them? Both
        // numbers come from the same frame, so scene and lens cancel out.
        let deficit = (a.metrics.sharpness_peak - st.main_sharpness).max(0.0);
        let focus = 1.0 - (deficit / 0.35).clamp(0.0, 1.0);

        // Subject sharp against a soft background means a long lens wide open
        // on a real subject — the look of a keeper.
        let isolation =
            ((st.main_sharpness - st.background_sharpness) / 0.25).clamp(0.0, 1.0);

        // A second sizeable subject is a bonus (a duel, a group), but a crowd
        // of specks is not, so this is driven by the *largest* other subject.
        let company = ((st.total_area - st.main_area) * 12.0).clamp(0.0, 1.0);

        100.0 * (0.55 * prominence + 0.27 * focus + 0.12 * isolation + 0.06 * company)
    } else {
        // Nothing recognisable in frame. Not damning — it might be a detail
        // shot, a boot, a ball in the net — but it is rarely the keeper, so
        // sit it below the middle rather than rejecting it outright.
        42.0
    };

    if norms.scenery {
        // A tilted horizon is obvious to a viewer and trivial to correct, so
        // it is worth naming. Only when there is actually a horizon to tilt.
        let tilt = a.metrics.horizon_tilt.abs();
        if a.metrics.horizon_strength > 0.18 && tilt > 1.2 {
            let p = (tilt / 6.0).clamp(0.0, 1.0) * 14.0;
            a.reasons.push(Reason {
                code: "tilted".into(),
                label: format!("Horizon off level by {tilt:.1}°"),
                severity: if tilt > 3.0 { Severity::Bad } else { Severity::Warn },
                delta: -p,
            });
            comp_extra -= p;
        }
        if a.metrics.detail_coverage > 0.7 {
            a.reasons.push(Reason {
                code: "deep-focus".into(),
                label: "Sharp corner to corner".into(),
                severity: Severity::Good,
                delta: 0.0,
            });
        } else if a.metrics.detail_coverage < 0.25 {
            a.reasons.push(Reason {
                code: "thin-focus".into(),
                label: "Little of the frame is sharp".into(),
                severity: Severity::Warn,
                delta: 0.0,
            });
        }
    } else if has_subject {
        let pct = st.main_area * 100.0;
        if pct >= 8.0 {
            a.reasons.push(Reason {
                code: "big-subject".into(),
                label: format!("Subject fills {pct:.0}% of frame"),
                severity: Severity::Good,
                delta: 0.0,
            });
        } else if pct < 1.0 {
            a.reasons.push(Reason {
                code: "distant".into(),
                label: format!("Subject is distant ({pct:.1}% of frame)"),
                severity: Severity::Warn,
                delta: 0.0,
            });
        }
        let deficit = (a.metrics.sharpness_peak - st.main_sharpness).max(0.0);
        if deficit > 0.3 {
            a.reasons.push(Reason {
                code: "subject-soft".into(),
                label: "Focus missed the subject".into(),
                severity: Severity::Bad,
                delta: 0.0,
            });
        }
    } else if s.w_subject > 0.15 {
        a.reasons.push(Reason {
            code: "no-subject".into(),
            label: "No recognisable subject".into(),
            severity: Severity::Warn,
            delta: 0.0,
        });
    }


    // --- Composition of the final score ----------------------------------
    //
    // Sharpness sets the baseline and everything else deducts from it. An
    // earlier version averaged all four components, which failed badly on real
    // material: exposure and framing sit at 100 for most competent frames, so
    // averaging dragged every photo into the top decile and the score sorted
    // nothing. Deduction also matches how the reasons list already reads —
    // each entry is a specific thing wrong, with a point cost.
    // Blend the technical baseline with the subject baseline. At w_subject = 0
    // this is the old purely-technical behaviour, which is the right thing for
    // subjects COCO has never heard of.
    let w = s.w_subject.clamp(0.0, 1.0);
    let blended = sharp_component * (1.0 - w) + subject_component * w;
    let base = 80.0 + (blended - 80.0) * s.w_sharpness;
    let penalty = (100.0 - exposure_component) * s.w_exposure
        + (100.0 - face_component) * s.w_faces
        + (100.0 - comp) * s.w_composition
        + motion_penalty
        // Scene-level faults (a tilted horizon) are already scaled to points.
        - comp_extra;

    let final_score = (base - penalty).clamp(0.0, 100.0);
    a.score = final_score;

    a.verdict = if disqualified {
        Verdict::Reject
    } else if final_score >= s.keep_threshold {
        Verdict::Keep
    } else if final_score < s.reject_threshold {
        Verdict::Reject
    } else {
        Verdict::Maybe
    };
}

/// Demote the runners-up in each burst, after grouping has picked a winner.
///
/// This runs separately from `score_photo` because a frame's fate depends on
/// its neighbours, and neighbours are only known once every frame has a score.
pub fn apply_group_policy(items: &mut [Analysis], s: &ScoreSettings) {
    if !s.keep_one_per_group {
        return;
    }

    // Collect group members first so we can reason about each group as a whole.
    let mut by_group: std::collections::HashMap<u32, Vec<usize>> = std::collections::HashMap::new();
    for (i, a) in items.iter().enumerate() {
        if let Some(g) = a.group_id {
            by_group.entry(g).or_default().push(i);
        }
    }

    for (_, members) in by_group {
        if members.len() < 2 {
            continue;
        }
        let best_score = members
            .iter()
            .map(|&i| items[i].score)
            .fold(f32::MIN, f32::max);

        for &i in &members {
            if items[i].is_group_best {
                items[i].reasons.push(Reason {
                    code: "group-best".into(),
                    label: format!("Best of {} similar", members.len()),
                    severity: Severity::Good,
                    delta: 0.0,
                });
                continue;
            }

            // If a runner-up is essentially as good as the winner, say so
            // rather than pretending the choice was clear-cut. The user may
            // legitimately prefer it, and a near-tie is a weak reason to reject.
            let gap = best_score - items[i].score;
            let near_tie = gap < 3.0;

            items[i].reasons.push(Reason {
                code: "duplicate".into(),
                label: if near_tie {
                    format!("Near-identical to the pick (only {gap:.1} pts behind)")
                } else {
                    "Similar frame, a better one exists".into()
                },
                severity: if near_tie { Severity::Warn } else { Severity::Bad },
                delta: 0.0,
            });

            // Never override an explicit user decision.
            if items[i].user_pick.is_none() {
                items[i].verdict = if near_tie {
                    Verdict::Maybe
                } else {
                    Verdict::Reject
                };
            }
        }
    }
}

/// Re-run scoring, grouping and the group policy over an existing set.
/// Used whenever the user changes a setting.
pub fn rescore_all(items: &mut Vec<Analysis>, s: &ScoreSettings) {
    // Changing which classes count as "the subject" must re-score without
    // re-running the network, so the stats are recomputed from the stored
    // boxes here. Background sharpness needs the tile map, which is long gone,
    // so carry the value measured during analysis forward.
    for a in items.iter_mut() {
        let background = a.subject_stats.background_sharpness;
        a.subject_stats = crate::subjects::summarise(
            &mut a.subjects,
            &s.subject_classes,
            a.metrics.sharpness_peak,
            None,
        );
        if background > 0.0 {
            a.subject_stats.background_sharpness = background;
        }
    }

    let norms = compute_norms(items, s);
    for a in items.iter_mut() {
        score_photo(a, &norms, s);
    }
    crate::group::assign_groups(items, s);
    apply_group_policy(items, s);

    // Re-apply manual picks last so they always win.
    for a in items.iter_mut() {
        match a.user_pick {
            Some(Pick::Keep) => a.verdict = Verdict::Keep,
            Some(Pick::Reject) => a.verdict = Verdict::Reject,
            None => {}
        }
    }
}
