use rusqlite::Connection;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = match std::env::var("APPDATA") {
        Ok(d) => PathBuf::from(d).join("omnisearch-lite").join("file_index.db"),
        Err(_) => PathBuf::from("file_index.db"),
    };

    println!("Checking database at: {:?}", db_path);

    // Resolve and print priority folders
    unsafe {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{
            FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Pictures,
            SHGetKnownFolderPath, KF_FLAG_DEFAULT,
        };

        let get_folder = |guid, name: &str| {
            let path =
                SHGetKnownFolderPath(guid, KF_FLAG_DEFAULT, HANDLE::default()).map(|result| {
                    let mut len = 0;
                    while *result.0.add(len) != 0 {
                        len += 1;
                    }
                    let s = String::from_utf16_lossy(std::slice::from_raw_parts(result.0, len));
                    CoTaskMemFree(Some(result.0 as *const _));
                    PathBuf::from(s)
                });
            println!("Known folder {}: {:?}", name, path);
        };

        get_folder(&FOLDERID_Desktop, "Desktop");
        get_folder(&FOLDERID_Documents, "Documents");
        get_folder(&FOLDERID_Downloads, "Downloads");
        get_folder(&FOLDERID_Pictures, "Pictures");
    }

    if !db_path.exists() {
        println!("Database file does not exist!");
        return Ok(());
    }

    let conn = Connection::open(&db_path)?;

    // 0. Print indexer state table
    println!("\n--- Indexer State Table ---");
    let mut stmt = conn.prepare("SELECT key, value FROM indexer_state").ok();
    if let Some(ref mut stmt) = stmt {
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for r in rows {
            if let Ok((k, v)) = r {
                println!("  {} = {:?}", k, v);
            }
        }
    } else {
        println!("  (indexer_state table does not exist or cannot be queried)");
    }
    println!("---------------------------\n");

    // 1. Total files
    let total_files: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;
    println!("Total files in 'files' table: {}", total_files);

    // 2. Total FTS entries
    let total_fts: i64 = conn.query_row("SELECT COUNT(*) FROM files_fts", [], |row| row.get(0))?;
    println!("Total files in 'files_fts' table: {}", total_fts);

    // 3. Total images
    let total_images: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files WHERE extension IN ('png', 'jpg', 'jpeg', 'bmp', 'gif')",
        [],
        |row| row.get(0),
    )?;
    println!("Total images in 'files' table: {}", total_images);

    // Print first 20 images in DB
    println!("\n--- First 20 Images in DB ---");
    let mut stmt_imgs = conn.prepare(
        "SELECT path FROM files WHERE extension IN ('png', 'jpg', 'jpeg', 'bmp', 'gif') LIMIT 20",
    )?;
    let img_rows = stmt_imgs.query_map([], |row| row.get::<_, String>(0))?;
    for img in img_rows {
        if let Ok(path) = img {
            println!("Image Path: {}", path);
        }
    }

    // Let's count files by priority folder
    let count_folder = |conn: &Connection, folder_path: &str| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM files WHERE path LIKE ?",
            [format!("{}%", folder_path)],
            |row| row.get(0),
        )
        .unwrap_or(0)
    };
    if let Ok(profile) = std::env::var("USERPROFILE") {
        println!(
            "Total files in Desktop: {}",
            count_folder(&conn, &format!("{}\\Desktop", profile))
        );
        println!(
            "Total files in Documents: {}",
            count_folder(&conn, &format!("{}\\Documents", profile))
        );
        println!(
            "Total files in Downloads: {}",
            count_folder(&conn, &format!("{}\\Downloads", profile))
        );
        println!(
            "Total files in Pictures: {}",
            count_folder(&conn, &format!("{}\\Pictures", profile))
        );
    }
    // 4. Print latest 10 images in Pictures
    println!("\n--- Latest 10 Pictures in DB ---");
    let mut stmt = conn.prepare(
        "SELECT path, name, extension, modified, size FROM files \
         WHERE path LIKE '%Pictures%' \
         ORDER BY modified DESC LIMIT 10",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;

    for r in rows {
        if let Ok((path, name, ext, modified, size)) = r {
            // Check if it has FTS content
            let has_fts: i64 = conn.query_row(
                "SELECT COUNT(*) FROM files_fts WHERE path = ? AND length(content) > 0",
                [&path],
                |row| row.get(0),
            )?;
            let fts_content: String = if has_fts > 0 {
                conn.query_row(
                    "SELECT content FROM files_fts WHERE path = ?",
                    [&path],
                    |row| row.get(0),
                )?
            } else {
                "None".to_string()
            };

            println!(
                "Path: {}\nName: {}\nExt: {}\nModified: {}\nSize: {} bytes\nHas FTS: {} (Content snippet: {:?})\n",
                path, name, ext, modified, size, has_fts > 0, fts_content
            );
        }
    }

    // 5. Check if anything matches 'test native image ocr'
    println!("\n--- FTS Match test for 'test native image ocr' ---");
    let clean_query = "test* native* image* ocr*";
    let mut stmt = conn.prepare("SELECT path, content FROM files_fts WHERE files_fts MATCH ?")?;
    let fts_matches = stmt.query_map([clean_query], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut match_count = 0;
    for m in fts_matches {
        if let Ok((path, content)) = m {
            println!("Match path: {}\nContent: {:?}\n", path, content);
            match_count += 1;
        }
    }
    println!("Total FTS matches found: {}", match_count);

    // WalkDir test on Downloads with extraction
    println!("\n--- WalkDir test on Downloads with extraction ---");
    let downloads_path = std::env::var("USERPROFILE")
        .map(|p| std::path::PathBuf::from(p).join("Downloads"))
        .unwrap_or_default();
    if downloads_path.exists() {
        let walker = walkdir::WalkDir::new(&downloads_path).into_iter();
        for (i, entry) in walker.enumerate() {
            match entry {
                Ok(e) => {
                    let path = e.path();
                    let is_file = path.is_file();
                    println!("Found entry {}: {:?} (is_file={})", i, path, is_file);
                    if is_file {
                        let ext = path
                            .extension()
                            .and_then(|ex| ex.to_str())
                            .unwrap_or("")
                            .to_lowercase();
                        if ext == "docx" {
                            println!("  DOCX extraction skipped (OmniSearch Lite)");
                        } else if ext == "pdf" {
                            println!("  PDF extraction skipped (OmniSearch Lite)");
                        } else if ["png", "jpg", "jpeg", "bmp", "gif"].contains(&ext.as_str()) {
                            println!("  Testing image OCR on {:?}", path);
                            // We won't run full WinRT OCR here yet to keep it simple, but let's see if we can resolve it
                        }
                    }
                }
                Err(err) => {
                    println!("Entry {} error: {:?}", i, err);
                }
            }
        }
    } else {
        println!("Downloads path does not exist!");
    }

    Ok(())
}
