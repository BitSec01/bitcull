# Working on Culling

Automatic photo culling for sports and event shooters. Rust + Tauri backend
(all pixel work, all state), React front end (pure view layer). Windows and
Linux.

The user shoots RAW+JPEG, culls the JPEG proxies, and needs the **RAW**
filenames out the other end. The JPEG is never the deliverable — every export
path resolves proxy → RAW by file stem.

---

## Commands

```bash
npm install
npm run fetch-models        # models are gitignored; do this first
npm run app                 # dev, hot reload
npm run app:build           # installer

cd src-tauri
cargo test --release        # unit tests (fast)
cargo run --release --example cull -- ../test_photos [limit]   # THE tool, see below
cargo run --release --example gaps -- ../test_photos           # burst timing analysis
```

`examples/cull.rs` runs the real pipeline over real photographs and prints the
score distribution, raw Laplacian percentiles, subject sizes, eye-state
breakdown, groupings, and the best/worst frames with reasons.

**Run it before and after any scoring change.** Every scoring bug in this
project's history was found by looking at its output, and none were visible
from reading the code.

---

## Architecture

```
src-tauri/src/
  decode.rs     DCT-scaled JPEG decode, orientation, resize, crop
  metrics.rs    sharpness map, region_sharpness, exposure, motion blur, noise
  hash.rs       pHash / dHash / 8x8 signature
  subjects.rs   YOLOX subject detection, prominence, isolation  <- the important one
  faces.rs      YuNet + eye state (optional, off by default), model lookup
  group.rs      burst and near-duplicate grouping (union-find)
  score.rs      weighting, verdicts, reasons, group policy
  pipeline.rs   parallel orchestration, progress, thumbnails
  export.rs     proxy->RAW resolution, copy/move/XMP/CSV
  cache.rs      on-disk result + thumbnail cache (bump CACHE_VERSION on metric changes)
  update.rs     GitHub release check
src/
  App.tsx       layout, filters, selection, stacking, keyboard
  fit.ts        mapping normalised boxes onto contain/cover images
  components/   grid, detail panel, loupe, export, cloud, sidebar
```

One decode per photo at `pipeline::WORK_SIZE` (3072 px) feeds every measurement
and the thumbnail. Decoding dominates the runtime; never add a second pass
without a good reason.

---

## Domain knowledge that is easy to get wrong

These are all real bugs that shipped and had to be fixed. They look correct in
code review and are only visible in the data.

**Frame-wide sharpness is anti-correlated with photo quality.** A wide record
shot is full of in-focus grass, crowd and treeline, so it scores *higher* on any
whole-frame measure than a tight action frame with a thrown-out background. The
first version's top-rated photo of an 872-frame shoot had a subject filling
0.4% of frame. Judge the **subject**, not the frame.

**Big boxes read soft.** Most of a person's bounding box is smooth clothing, so
a tack-sharp player filling a quarter of the frame measures softer than a
distant one whose box is mostly grass. Use `metrics::region_sharpness` (tiled,
85th percentile), never a plain variance over a large region. Same trap as
"skin is smooth" for faces.

**The biggest subject is not always the subject.** On a touchline the largest
box is regularly a linesman two metres from the lens, well outside the plane of
focus. `subjects::main_rank` picks by size **and** focus together.

**Labelling is absolute; ranking is relative.** An early version normalised
sharpness against the shoot's 10th–90th percentile and then used that to
*label* frames, so the relatively worst frame in a perfectly sharp set was
called "badly out of focus". Absolute scale decides accusations; percentile
rank only spreads the scores for sorting.

**Score by deduction, not by averaging.** Averaging four components put every
photo in the top decile, because exposure and framing sit at 100 for most
competent frames. Sharpness/subject sets a baseline and everything else
subtracts.

**"Can't tell" is not "closed".** Treating a flat, low-contrast eye crop as
evidence of a blink made the app declare two thirds of all faces to be
blinking. Return `None`/`Unknown` when the crop is unreadable. Thresholds are
deliberately asymmetric: a false blink costs a keeper and the user's trust; a
missed blink merely survives to be looked at.

**Measure, do not guess, calibration constants.** Burst grouping used a 2 s
window until `examples/gaps.rs` showed continuous drive fires every
0.06–0.12 s with a near-empty band before 0.25 s. At 2 s the longest unbroken
run was 54 frames — a whole attacking move in one stack. It is now 0.5 s.

**Scenery is a different question, not a lower score.** Judging a landscape as
though it should have contained a subject rejected 90% of a landscape folder
and bunched every score between 44 and 50 — the `no-subject` penalty fired on
everything, so nothing was distinguishable from anything else. Frames with
nothing recognisable are judged on frame-wide sharpness, tonal range and
horizon level instead. The decision is made **per shoot** (`score::is_scenery`,
under 20% of frames carrying a subject), never per frame: in a football set a
frame where you missed the players is a failure, not a landscape.

**RAW means the embedded preview, not the sensor data.** Demosaicing would be
slow and would not look like the photograph. `raw.rs` scans the container for
JPEG streams and validates each by parsing it, which is format-agnostic —
TIFF-based CR2/NEF/ARW and BMFF-based CR3 all work through the same path. The
validation is what makes it safe: sensor data is full of `FF D8 FF` sequences
that do not parse as JPEG. CR3 also needs its EXIF read from inside the
preview, since ordinary EXIF parsers cannot read its container.

**`WORK_SIZE` changes the Laplacian scale.** Laplacian variance is
resolution-dependent; a smaller working size raises it through aliasing. The
constants in `metrics.rs` (`SHARP_LOG_LO` / `SHARP_LOG_HI`) assume 3072 px.
Change one, re-run `examples/cull.rs` and re-derive the other.

---

## Conventions

- Comments explain **why**, especially where a value was measured rather than
  chosen. Do not annotate the obvious.
- Every score deduction is a `Reason` with a human-readable label. The score is
  never a black box; the UI must be able to say why a frame was cut.
- UI controls are named by **outcome**, not mechanism. "How harsh should the
  cull be?", not "verdict thresholds". Weights read Off / Barely / Some / A lot,
  not `0.6×`. Jargon lives in the collapsed Advanced section. See
  `components/Sidebar.tsx`.
- Nothing destructive by default. The most aggressive export is a move, and it
  refuses to overwrite.
- Manual picks always beat computed verdicts, including inside a burst.

---

## Traps

**Never rewrite a source file with PowerShell `-replace` + `Set-Content`.** It
reads UTF-8 as ANSI and silently turns `—` into `â€"`, which then fails to
compile with `unknown start of token`. Use the editing tools. If a file is
already mangled, round-trip it: encode to CP1252, decode as UTF-8.

**Tauri v2 denies plugin calls without a capability.** `capabilities/default.json`
grants `core:default`, `dialog:allow-open`, `dialog:allow-save`,
`opener:default`. Without it the folder picker rejects silently and the button
appears dead. Always `.catch()` a plugin call and surface the error.

**Model lookup must search per file, not per directory.** The build copies
resources next to the executable, so a stale `target/release/models/` can
shadow the real one and hide a model that is present. See
`faces::locate_model`.

**Kill any running `culling.exe` before `cargo build --release`**, or the link
step fails with "Access is denied".

**`tract` cannot load NanoDet** (`Resize` with `pytorch_half_pixel`). YOLOX-S
is the detector partly for that reason.

**Bump `cache::CACHE_VERSION`** whenever the meaning of a stored metric
changes, or stale results silently mix with new ones.

---

## Releasing

Models are bundled into the installer (~38 MB on Windows) so users download one
file and it works offline. Tag to release:

```bash
npm version minor && git push --follow-tags
```

`.github/workflows/release.yml` builds Windows, macOS (both architectures) and
Linux, fetches models, verifies they are not truncated, and drafts a GitHub
release.

Licensing: the project is **GPL-3.0-or-later**. Bundled models keep their own
licences (YOLOX Apache-2.0, YuNet MIT), both compatible and both reproduced in
`THIRD-PARTY-NOTICES.md`. Apache-2.0 is compatible with GPL-3.0 but **not**
GPL-2.0, so the project cannot be relicensed to GPL-2.0 while it bundles YOLOX.

Attribution is **BitSec01**. Do not add real names or personal email addresses
anywhere in the repository.
