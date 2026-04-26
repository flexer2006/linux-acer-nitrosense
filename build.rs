// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2024-2026 NitroSense Contributors

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    let fasm_src = "src/c_hw/port_io.asm";
    let fasm_obj = out_dir.join("port_io.o");

    println!("cargo:rerun-if-changed={fasm_src}");

    let fasm_status = Command::new("fasm").arg(fasm_src).arg(&fasm_obj).status();

    match fasm_status {
        Ok(status) if status.success() => {}
        Ok(status) => panic!("fasm failed with exit status: {status}"),
        Err(e) => {
            println!("cargo:warning=fasm not found ({e}). Assembly optimizations disabled.");
        }
    }

    let mut build = cc::Build::new();
    build
        .std("c2x")
        .include("src/c_hw/include")
        .file("src/c_hw/ec_lowlevel.c")
        .file("src/c_hw/msr_lowlevel.c")
        .warnings(true)
        .extra_warnings(true);

    if fasm_obj.exists() {
        build.object(&fasm_obj);
    }

    build.compile("nitro_hw");

    println!("cargo:rerun-if-changed=src/c_hw/ec_lowlevel.c");
    println!("cargo:rerun-if-changed=src/c_hw/msr_lowlevel.c");
    println!("cargo:rerun-if-changed=src/c_hw/include/nitro_hw.h");
}
