use crate::{ReleaseCapture, SetCapture, VK_ESCAPE};
use image::{codecs::png::PngEncoder, ColorType, ImageEncoder};
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

struct OverlayData {
    bmp: HBITMAP,
    screen_w: i32,
    screen_h: i32,
    points: Vec<POINT>,
    active: bool,
}

pub fn start_lens_search_async() {
    std::thread::spawn(|| unsafe {
        let Ok(hinst) = GetModuleHandleW(None) else {
            return;
        };
        if let Some((bmp, x, y, w, h)) = capture_screen() {
            if let Some(overlay) = create_overlay_window(hinst, bmp, x, y, w, h) {
                let _ = UpdateWindow(overlay);
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    let _ = DispatchMessageW(&msg);
                }
            } else {
                let _ = DeleteObject(bmp);
            }
        }
    });
}

unsafe fn capture_screen() -> Option<(HBITMAP, i32, i32, i32, i32)> {
    let screen_dc = GetDC(HWND::default());
    if screen_dc.is_invalid() {
        return None;
    }
    let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
    let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
    let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
    let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
    let bmp = CreateCompatibleBitmap(screen_dc, w, h);
    if bmp.is_invalid() {
        let _ = ReleaseDC(HWND::default(), screen_dc);
        return None;
    }
    let mem_dc = CreateCompatibleDC(screen_dc);
    if mem_dc.is_invalid() {
        let _ = DeleteObject(bmp);
        let _ = ReleaseDC(HWND::default(), screen_dc);
        return None;
    }
    let old = SelectObject(mem_dc, bmp);
    let _ = BitBlt(mem_dc, 0, 0, w, h, screen_dc, x, y, SRCCOPY);
    let _ = SelectObject(mem_dc, old);
    let _ = DeleteDC(mem_dc);
    let _ = ReleaseDC(HWND::default(), screen_dc);
    Some((bmp, x, y, w, h))
}

unsafe fn create_overlay_window(
    hinst: HMODULE,
    bmp: HBITMAP,
    screen_x: i32,
    screen_y: i32,
    screen_w: i32,
    screen_h: i32,
) -> Option<HWND> {
    let class: Vec<u16> = "omnisearch-lite-circle-search\0".encode_utf16().collect();
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(overlay_wndproc),
        hInstance: HINSTANCE(hinst.0),
        hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
        hbrBackground: HBRUSH::default(),
        lpszClassName: PCWSTR(class.as_ptr()),
        ..Default::default()
    };
    RegisterClassW(&wc);

    let data = Box::into_raw(Box::new(OverlayData {
        bmp,
        screen_w,
        screen_h,
        points: Vec::new(),
        active: false,
    }));

    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        PCWSTR(class.as_ptr()),
        PCWSTR::null(),
        WS_POPUP,
        screen_x,
        screen_y,
        screen_w,
        screen_h,
        None,
        None,
        HINSTANCE(hinst.0),
        Some(data as _),
    );
    match hwnd {
        Ok(h) => {
            let _ = SetWindowPos(
                h,
                HWND_TOPMOST,
                screen_x,
                screen_y,
                screen_w,
                screen_h,
                SWP_SHOWWINDOW,
            );
            let _ = SetForegroundWindow(h);
            Some(h)
        }
        Err(_) => {
            let _ = Box::from_raw(data);
            None
        }
    }
}

extern "system" fn overlay_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_CREATE {
            let cs = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            return LRESULT(0);
        }

        let data_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut OverlayData;
        if data_ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let data = &mut *data_ptr;

        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint_overlay(hwnd, data);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let x = loword(lp.0 as u32);
                let y = hiword(lp.0 as u32);
                data.points.clear();
                data.points.push(POINT { x, y });
                data.active = true;
                let _ = SetCapture(hwnd);
                let _ = InvalidateRect(hwnd, None, FALSE);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let x = loword(lp.0 as u32);
                let y = hiword(lp.0 as u32);
                if data.active {
                    let last = data.points.last().cloned();
                    if let Some(l) = last {
                        let dx = (l.x - x).abs();
                        let dy = (l.y - y).abs();
                        // Only add point if mouse moved at least 2 pixels to keep polygon size sane
                        if dx > 2 || dy > 2 {
                            data.points.push(POINT { x, y });
                        }
                    }
                }
                let _ = InvalidateRect(hwnd, None, FALSE);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if data.active {
                    let _ = ReleaseCapture();
                    data.active = false;
                    let x = loword(lp.0 as u32);
                    let y = hiword(lp.0 as u32);
                    data.points.push(POINT { x, y });

                    let mut min_x = i32::MAX;
                    let mut min_y = i32::MAX;
                    let mut max_x = i32::MIN;
                    let mut max_y = i32::MIN;
                    for p in &data.points {
                        min_x = min_x.min(p.x);
                        min_y = min_y.min(p.y);
                        max_x = max_x.max(p.x);
                        max_y = max_y.max(p.y);
                    }
                    let sx = min_x.max(0);
                    let sy = min_y.max(0);
                    let sw = (max_x - min_x).min(data.screen_w - sx);
                    let sh = (max_y - min_y).min(data.screen_h - sy);

                    if sw >= 8 && sh >= 8 && data.points.len() >= 3 {
                        if let Some(cropped) = crop_bitmap(data.bmp, sx, sy, sw, sh) {
                            process_image(cropped);
                        }
                        let _ = DestroyWindow(hwnd);
                        PostQuitMessage(0);
                    } else {
                        data.points.clear();
                        let _ = InvalidateRect(hwnd, None, FALSE);
                    }
                }
                LRESULT(0)
            }
            WM_KEYDOWN => {
                if wp.0 as u32 == VK_ESCAPE.0 as u32 {
                    let _ = DestroyWindow(hwnd);
                    PostQuitMessage(0);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                // Clear GWLP_USERDATA before freeing so any message still in flight for this
                // hwnd (WM_NCDESTROY, or anything queued before DestroyWindow was called) sees
                // a null data_ptr and safely no-ops via the check above, instead of
                // dereferencing freed memory.
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let data = Box::from_raw(data_ptr);
                let _ = DeleteObject(data.bmp);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

unsafe fn paint_overlay(hwnd: HWND, data: &OverlayData) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);

    let back_dc = CreateCompatibleDC(hdc);
    let back_bmp = CreateCompatibleBitmap(hdc, data.screen_w, data.screen_h);
    let old_back = SelectObject(back_dc, back_bmp);
    let src_dc = CreateCompatibleDC(hdc);
    let old_src = SelectObject(src_dc, data.bmp);

    let _ = BitBlt(
        back_dc,
        0,
        0,
        data.screen_w,
        data.screen_h,
        src_dc,
        0,
        0,
        SRCCOPY,
    );

    let shade_dc = CreateCompatibleDC(hdc);
    let shade_bmp = CreateCompatibleBitmap(hdc, data.screen_w, data.screen_h);
    let old_shade = SelectObject(shade_dc, shade_bmp);
    let _ = PatBlt(shade_dc, 0, 0, data.screen_w, data.screen_h, BLACKNESS);
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 96,
        AlphaFormat: 0,
    };
    let _ = AlphaBlend(
        back_dc,
        0,
        0,
        data.screen_w,
        data.screen_h,
        shade_dc,
        0,
        0,
        data.screen_w,
        data.screen_h,
        blend,
    );
    let _ = SelectObject(shade_dc, old_shade);
    let _ = DeleteObject(shade_bmp);
    let _ = DeleteDC(shade_dc);

    if data.points.len() > 1 {
        let glow = CreatePen(PS_SOLID, 8, rgb(30, 34, 42));
        let old_pen = SelectObject(back_dc, glow);
        let _ = Polyline(back_dc, &data.points);
        let _ = SelectObject(back_dc, old_pen);
        let _ = DeleteObject(glow);

        let pen = CreatePen(PS_SOLID, 4, rgb(245, 248, 255));
        let old_pen = SelectObject(back_dc, pen);
        let _ = Polyline(back_dc, &data.points);
        let _ = SelectObject(back_dc, old_pen);
        let _ = DeleteObject(pen);
    }

    let _ = BitBlt(
        hdc,
        0,
        0,
        data.screen_w,
        data.screen_h,
        back_dc,
        0,
        0,
        SRCCOPY,
    );

    let _ = SelectObject(src_dc, old_src);
    let _ = DeleteDC(src_dc);
    let _ = SelectObject(back_dc, old_back);
    let _ = DeleteObject(back_bmp);
    let _ = DeleteDC(back_dc);
    let _ = EndPaint(hwnd, &ps);
}

unsafe fn crop_bitmap(src: HBITMAP, x: i32, y: i32, w: i32, h: i32) -> Option<HBITMAP> {
    let screen_dc = GetDC(HWND::default());
    let src_dc = CreateCompatibleDC(screen_dc);
    let old_src = SelectObject(src_dc, src);
    let dst = CreateCompatibleBitmap(screen_dc, w, h);
    if dst.is_invalid() {
        let _ = SelectObject(src_dc, old_src);
        let _ = DeleteDC(src_dc);
        let _ = ReleaseDC(HWND::default(), screen_dc);
        return None;
    }
    let dst_dc = CreateCompatibleDC(screen_dc);
    let old_dst = SelectObject(dst_dc, dst);
    let _ = BitBlt(dst_dc, 0, 0, w, h, src_dc, x, y, SRCCOPY);
    let _ = SelectObject(dst_dc, old_dst);
    let _ = SelectObject(src_dc, old_src);
    let _ = DeleteDC(dst_dc);
    let _ = DeleteDC(src_dc);
    let _ = ReleaseDC(HWND::default(), screen_dc);
    Some(dst)
}

unsafe fn process_image(hbmp: HBITMAP) {
    let Some((rgba, w, h)) = bitmap_to_rgba(hbmp) else {
        let _ = DeleteObject(hbmp);
        return;
    };
    let _ = DeleteObject(hbmp);
    let mut png = Vec::new();
    if PngEncoder::new(&mut png)
        .write_image(&rgba, w, h, ColorType::Rgba8)
        .is_err()
    {
        return;
    }
    upload_to_lens(png);
}

unsafe fn bitmap_to_rgba(hbmp: HBITMAP) -> Option<(Vec<u8>, u32, u32)> {
    let screen_dc = GetDC(HWND::default());
    let mem_dc = CreateCompatibleDC(screen_dc);
    let old = SelectObject(mem_dc, hbmp);
    let mut bmp = BITMAP::default();
    if GetObjectW(
        hbmp,
        std::mem::size_of::<BITMAP>() as i32,
        Some(&mut bmp as *mut _ as *mut _),
    ) == 0
    {
        let _ = SelectObject(mem_dc, old);
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(HWND::default(), screen_dc);
        return None;
    }
    let w = bmp.bmWidth as u32;
    let h = bmp.bmHeight as u32;

    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w as i32;
    bmi.bmiHeader.biHeight = -(h as i32);
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB.0;

    let mut bgra = vec![0u8; (w * h * 4) as usize];
    let ok = GetDIBits(
        mem_dc,
        hbmp,
        0,
        h,
        Some(bgra.as_mut_ptr() as *mut _),
        &mut bmi,
        DIB_RGB_COLORS,
    );
    if ok == 0 {
        let _ = SelectObject(mem_dc, old);
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(HWND::default(), screen_dc);
        return None;
    }

    let _ = SelectObject(mem_dc, old);
    let _ = DeleteDC(mem_dc);
    let _ = ReleaseDC(HWND::default(), screen_dc);

    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }

    Some((bgra, w, h))
}

fn upload_to_lens(png: Vec<u8>) {
    let html_path =
        std::env::temp_dir().join(format!("omnisearch_lite_lens_{}.html", std::process::id()));
    let html = lens_upload_html(&base64_encode(&png));
    if std::fs::write(&html_path, html).is_err() {
        return;
    }
    unsafe {
        open_path(&html_path);
    }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(60));
        let _ = std::fs::remove_file(html_path);
    });
}

fn lens_upload_html(b64_png: &str) -> String {
    format!(
        r#"<!doctype html>
<html>
<body>
<form id="f" action="https://lens.google.com/v3/upload?hl=en" method="POST" enctype="multipart/form-data">
  <input id="i" name="encoded_image" type="file" accept="image/*">
</form>
<script>
const raw = atob("{}");
const bytes = new Uint8Array(raw.length);
for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
const file = new File([new Blob([bytes], {{ type: "image/png" }})], "selection.png", {{ type: "image/png" }});
const dt = new DataTransfer();
dt.items.add(file);
document.getElementById("i").files = dt.files;
document.getElementById("f").submit();
</script>
</body>
</html>"#,
        b64_png
    )
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((triple >> 18) & 0x3f) as usize] as char);
        out.push(CHARS[((triple >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            CHARS[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            CHARS[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

unsafe fn open_path(path: &std::path::Path) {
    let wide: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let _ = ShellExecuteW(
        HWND::default(),
        windows::core::w!("open"),
        PCWSTR(wide.as_ptr()),
        PCWSTR::null(),
        PCWSTR::null(),
        SW_SHOWNORMAL,
    );
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | ((g as u32) << 8) | ((b as u32) << 16))
}

fn loword(dw: u32) -> i32 {
    (dw & 0xFFFF) as i16 as i32
}

fn hiword(dw: u32) -> i32 {
    ((dw >> 16) & 0xFFFF) as i16 as i32
}
