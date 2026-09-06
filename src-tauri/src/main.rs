// Culling — automatic photo culling for sports and event shooters.
// Copyright (C) 2026 BitSec01
//
// This program is free software: you can redistribute it and/or modify it
// under the terms of the GNU General Public License as published by the Free
// Software Foundation, either version 3 of the License, or (at your option)
// any later version. See the LICENSE file, or <https://www.gnu.org/licenses/>.

// Hide the console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    culling_lib::run()
}
