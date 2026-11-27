# 🗡️ Pakem Blade Engine (`pakem-blade`)

<p align="center">
  <img src="https://raw.githubusercontent.com/pakem-rs/pakem/1.x/resources/images/logo.svg" alt="Pakem Framework Logo" width="120">
</p>

> **"Look like Laravel, Run at Rust Speed."**  
> Pure, blazingly fast, and standalone Pakem Blade templating engine written natively in Rust.

[![Crates.io](https://img.shields.io/crates/v/pakem-blade.svg)](https://crates.io/crates/pakem-blade)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust 2024](https://img.shields.io/badge/Rust-2024_Edition-orange.svg)](https://www.rust-lang.org/)

`pakem-blade` menghadirkan sintaks Blade Laravel 100% native di Rust tanpa ketergantungan runtime PHP. Dapat digunakan langsung di framework web Rust populer seperti **Pakem Framework**, **Axum**, **Actix-web**, **Poem**, atau **Rocket**.

---

## ✨ Fitur Utama

- 🎨 **Layout Inheritance**: `@extends('layouts.app')`, `@section('content')`, `@yield('content')`.
- 🔂 **Looping & Kontrol Alur**: `@foreach`, `@forelse`, `@empty`, `@while`, `@for`, `@if`, `@elseif`, `@unless`, `@switch`.
- 📦 **Modern Blade Components**: Self-closing `<x-alert type="warning" />`, block component `<x-card>...</x-card>`, dan named slots `<x-slot:header>`.
- 🧩 **Sub-View `@include` dengan Parameter**: `@include('view', ['key' => $val])` dan `@include('view', compact('user'))`.
- 🛡️ **Production Shield**: Direktif `@productionShield` (Anti-Inspect element, Anti-DevTools shortcut interception, loop anti-debugger, dan CSS user-select blocker).
- ⚡ **HTML Minification**: Minifikasi HTML terintegrasi secepat kilat (`.minify()`).
- 🚀 **Asset Bundler**: `@vite(['resources/css/app.css', 'resources/js/app.js'])` dan helper `mix('...')`.
- 🪄 **Filter Pipeline**: `{{ $nominal | rupiah }}`, `{{ $title | slug | uppercase }}`, `{{ $body | limit:100 }}`.
- 🔌 **Extensible Plugin Runner**: Dukungan eksekusi WebAssembly Sandbox (`@wasm`) dan Indonesian Script (`@eval_is`).

---

## 📦 Pemasangan

Tambahkan ke `Cargo.toml`:

```toml
[dependencies]
pakem-blade = "1.0.5"
serde_json = "1.0"
```

---

## 🚀 Penggunaan Cepat

```rust
use pakem_blade::{BladeEngine, BladeContext};

fn main() {
    let mut engine = BladeEngine::new();

    // Simpan template ke memory (atau letakkan di folder resources/views/**/*.blade.rs)
    engine.register_template("greeting", "<h1>Halo, {{ $nama }}!</h1>");

    let mut context = BladeContext::new();
    context.insert("nama", "Kawan Rust");

    let html = engine.render("greeting", &context).unwrap();
    println!("{}", html); // <h1>Halo, Kawan Rust!</h1>
}
```

---

## 📄 Lisensi

Dilisensikan di bawah [MIT License](LICENSE).
