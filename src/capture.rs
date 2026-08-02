name: build

on:
  push:
  workflow_dispatch:

jobs:
  windows:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable

      - name: Cache
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.toml') }}

      - name: Compilar
        run: cargo build --release

      - name: Subir artefacto
        uses: actions/upload-artifact@v4
        with:
          name: mvc-netmon
          path: target/release/mvc-netmon.exe
