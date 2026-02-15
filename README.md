# OmniSearch

High-performance Windows desktop file search built with Tauri v2, Rust, and C++.
OmniSearch indexes NTFS metadata directly through USN/MFT APIs for fast global search with advanced filters.

![OmniSearch Logo](src-tauri/icons/icon.png)
![OmniSearch Architecture](docs/images/omnisearch-architecture.svg)

> Optional: add a UI screenshot as `docs/images/omnisearch-ui.png` and reference it in this README for a live app preview.

## Features

- Native Windows indexing engine in C++ using `DeviceIoControl` + USN/MFT enumeration.
- Rust FFI bridge exposing Tauri commands: `start_indexing`, `index_status`, `search_files`.
- React + TypeScript "Spotlight-style" UI with non-blocking async search.
- Filter support by extension, file size range, and created date range.
- MSI installer output for distribution.
- Windows manifest configured to request Administrator privileges (`requireAdministrator`) for raw volume access.

## Tech Stack

- Frontend: React 19, TypeScript, Vite
- Desktop shell: Tauri v2
- Bridge: Rust (`tauri`, `serde`, `cc`)
- Native engine: C++ (Win32 API, NTFS USN/MFT)
- Installer: WiX/MSI via Tauri bundle

## Repository Structure

```text
omni-search/
|- src/                          # React UI
|  |- App.tsx
|  |- App.css
|  `- main.tsx
|- public/
|  `- app-icon.png               # Frontend favicon
|- src-tauri/
|  |- cpp/
|  |  `- scanner.cpp             # Native NTFS scanner + search engine
|  |- src/
|  |  |- lib.rs                  # Tauri commands + FFI bindings
|  |  `- main.rs
|  |- build.rs                   # Compiles C++ and embeds Windows manifest
|  |- windows-app-manifest.xml   # requireAdministrator for volume access
|  |- tauri.conf.json            # App/bundle config
|  `- icons/
|- docs/
|  `- images/
|     |- omnisearch-architecture.svg
|     `- README.md
|- index.html
|- package.json
`- README.md
```

## How It Works

1. UI calls Tauri commands from React using `@tauri-apps/api`.
2. Rust command layer forwards calls to C++ through `extern "C"` FFI.
3. C++ scanner reads NTFS metadata via USN/MFT (`DeviceIoControl`).
4. Search results are serialized to JSON and returned to the UI.

## Requirements

- Windows 10/11 (NTFS volume)
- Node.js 20+ and npm
- Rust stable toolchain (`x86_64-pc-windows-msvc`)
- Visual Studio 2022 C++ Build Tools (`Desktop development with C++`)
- WebView2 Runtime (normally preinstalled on Windows 11)

## Quick Start (Development)

```powershell
cd e:\omni-search
npm install
cd src-tauri
cargo check
cd ..
npm run tauri dev
```

Important:

- The app needs Administrator privileges to read `\\.\C:` for USN/MFT data.
- If indexing fails with "Unable to open volume", run the app elevated or use the packaged build with UAC prompt.

## Build Installers (Distribution)

Build MSI:

```powershell
cd e:\omni-search
npx tauri build -b msi
```

Output path:

- `src-tauri/target/release/bundle/msi/omni-search_0.1.0_x64_en-US.msi`

Build EXE installer (NSIS):

```powershell
npx tauri build -b nsis
```

Output path:

- `src-tauri/target/release/bundle/nsis/`

## Customize App Icon / Branding

Generate all required Tauri icons from one square source image:

```powershell
npx tauri icon .\path\to\your-logo-1024.png --output .\src-tauri\icons
```

Update visible app metadata in `src-tauri/tauri.conf.json`:

- `productName`
- `app.windows[0].title`

## Troubleshooting

- `Unable to open volume`:
  - Run as Administrator.
  - Confirm target drive is NTFS: `fsutil fsinfo volumeinfo C:`.
- `cl.exe not found`:
  - Install Visual Studio C++ Build Tools and reopen terminal.
- App still shows old icon:
  - Regenerate icons, run `cargo clean`, rebuild, and restart Explorer (Windows icon cache).

## Open-Source Publishing Checklist

- Add a `LICENSE` file (MIT/Apache-2.0 recommended for broad reuse).
- Add `docs/images/omnisearch-ui.png` screenshot.
- Verify `.gitignore` excludes generated artifacts (`node_modules`, `dist`, `src-tauri/target`).
- Tag a release after first stable MSI build.

## Contributing

1. Fork the repo.
2. Create a feature branch.
3. Run checks (`cargo check`, `npm run build`).
4. Open a PR with test notes and benchmark notes if scanner logic changed.
"# omni-search" 
