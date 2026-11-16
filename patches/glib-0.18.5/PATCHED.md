# Patched glib 0.18.5

Unmodified crates.io `glib` 0.18.5 (MIT, see `LICENSE`), except `src/variant_iter.rs`:
`VariantStrIter::impl_get` passes its out-pointer as `&mut p` instead of `&p`.
This is the upstream fix for GHSA-wrw7-89jp-8q8g / RUSTSEC-2024-0429, first released in glib 0.20.0.
The `benches/` and `tests/` folders and the `[[bench]]`/`[[test]]` manifest entries were removed because they are not built here.

Tauri 2.12 (wry/tao/webkit2gtk) still depends on the gtk-rs 0.18 GTK3 stack, so no compatible patched release exists.
Delete this folder and the root `[patch.crates-io]` entry once the Tauri stack moves to glib >= 0.20.
