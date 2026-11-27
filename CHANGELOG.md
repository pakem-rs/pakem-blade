# 📝 Changelog — `pakem-blade`

Semua perubahan penting pada crate **Pakem Blade Engine** (`pakem-blade`) didokumentasikan di dalam berkas ini.

Format pencatatan mengacu pada [Keep a Changelog](https://keepachangelog.com/en/1.0.0/) dan mengikuti standar [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [1.0.5] - 2026-11-27

### 🚀 Standalone Crate Extraction & Crates.io Release
- **Modularisasi Mesin Template**:
  - Diterbitkan sebagai crate mandiri berkecepatan tinggi di [crates.io/crates/pakem-blade](https://crates.io/crates/pakem-blade).
  - Mengadopsi arsitektur terisolasi dengan dependensi minimal: `regex`, `serde`, `serde_json`, `chrono`, dan `indonesian-script`.
- **Ekosistem Fitur Lengkap**:
  - **Layout Inheritance**: `@extends`, `@section`, `@yield`, `@hasSection`.
  - **Modern Blade Components**: Self-closing `<x-alert ... />`, block component `<x-card>...</x-card>`, dan named slots (`<x-slot:header>...</x-slot:header>`).
  - **Parameter Sub-View `@include`**: Mendukung array binding `['key' => $val]` dan `compact(...)` dengan isolasi sub-context.
  - **Asset Bundler Integration**: Direktif `@vite(['...'])` dan Laravel Mix helper `mix('...')`.
  - **Production Template Shield**: Direktif `@productionShield` (Anti-Inspect element, pemblokir pintasan DevTools/View Source, anti-debugger interval loop, deteksi resizing jendela browser, dan proteksi CSS `user-select: none`).
  - **HTML Minification**: Helper `.minify()` dan pemangkas whitespace/komentar HTML otomatis.
  - **Pipeline Filter Berantai**: Filter native Rust (`rupiah`, `slug`, `uppercase`, `limit`, dll.) dengan dukungan fallback filter kustom.
  - **BladePluginRunner Interface**: Abstraksi trait `BladePluginRunner` untuk eksekusi WebAssembly Sandbox (`@wasm`) dan Indonesian Script (`@eval_is`).
