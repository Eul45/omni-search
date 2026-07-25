# OmniSearch v0.1.15 & OmniSearch Lite v1.0.0

## OmniSearch Lite v1.0.0
Introducing OmniSearch Lite, a standalone lightweight desktop launcher.

### Features
* Circle to Search (Screen Lens): Freeze screen and drag a selection box to search visually (Lens icon).
* AI Web Search Shortcuts:
  * chatgpt: <prompt> - Open ChatGPT with prompt
  * claude: <prompt> - Open Claude AI with prompt
  * grok: <prompt> - Open Grok AI with prompt
  * eyux: <prompt> - Open EyuX AI with prompt
  * gemini: - Open Google Gemini
* Built-in AI Chat Assistant: Lightweight conversational chat window (@ AI:).
* Local File Search & Indexing: FTS5 file indexing with customizable include/exclude folder management.
* Browser History & Bookmarks: Direct search across browser history and bookmarks (history:, bookmarks:).
* Phone Sync: Connect to OmniSearch mobile app for file transfer and device pairing.
* Built-in Plugins: Calculator, Color Picker, and Text Expansions (Snippets).
* Customizable Hotkeys & Settings: Custom global hotkeys, theme selection, font sizing, and window placement.
---
## OmniSearch Desktop v0.1.15

### New Features & Improvements
*   Added automatic update support.
*   Bulk Selection Mode: Long-press any file result to enter multi-select mode. Select multiple files and perform powerful bulk actions:
    *   Bulk Rename - Find & Replace text in filenames, or Add Prefix/Suffix to all selected files at once.
    *   Bulk Send to Phone - Queue multiple files for transfer to your connected mobile device in one action.
    *   Bulk Delete - Send multiple files to the Recycle Bin (or permanently delete) with a single confirmation.
    *   Bulk Copy Paths - Copy all selected file paths to clipboard.
*   Folder Picker in Search Bar: A new folder icon button in the search input lets you open the native Windows folder browser dialog, select any folder, and instantly search within that specific directory.
*   Sync Tab in Quick Window Mode: Access the Mobile Sync tab from Quick Window mode via a new "Sync" toggle button in the header.
*   Large File Downloads: Unlocked the ability to download larger file sizes directly from remote search results.
*   Sync Server Stability: Improved backend memory management and stability, ensuring smooth and reliable transfers when pushing massive files to connected mobile devices.
*   Enhanced Error Handling: Fortified edge-case transfer scenarios. The desktop app now recovers gracefully without crashing if a connected mobile device unexpectedly drops its connection mid-transfer.
*  Bug Fix: Fixed an issue where the Explorer right-click "Send to OmniSearch Phone" context menu option remained in the Windows Registry after uninstalling the app.

---

## Mobile Release: v1.1

### New Features
*   OmniSearch Home Screen Widgets! Added 5 highly customizable widgets featuring transparency and vibrant coloring controls.
    *   *Smart Search Widget:* Search your phone's internal storage *or* your connected Desktop files directly from your home screen!
    *   *Music Player Widget:* Control your tunes straight from your home screen.
*   Revamped Music Player: A massive overhaul to the built-in music experience! Enjoy a much better layout, dedicated album views, automatic cover image extraction from audio files, and new bulk actions (just long-press to multi-select).
*   Movable Transfer Cards: Incoming and outgoing file transfer progress cards are now completely draggable! Just long-press and drag them up or down out of the way.
*   Expanded Recent Files: The "Recent Files" section on the dashboard now surfaces all file types - APKs, EXEs, archives, documents transferred via USB, Bluetooth, OmniSearch sync, and third-party apps. Previously limited to 15 mostly-media files, it now shows up to 40 recent files from across your entire internal storage.