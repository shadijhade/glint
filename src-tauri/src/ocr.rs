//! Text in pictures, read by Windows' own OCR (Windows.Media.Ocr, in the languages of the user's profile
//! that have it): Copy text in a picture's preview, and Text's Images too option for Documents search,
//! read in its workers (doctext.rs). On a thread of the multithreaded apartment, where its async calls
//! are waited for, as thumbs.rs reads PDFs.

use windows::core::HSTRING;
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// The pictures OCR reads: what Windows decodes (WebP, HEIC and AVIF with Microsoft's extensions for them).
pub const PICTURES: &[&str] = &["png", "jpg", "jpeg", "jpe", "jfif", "bmp", "gif", "tif", "tiff", "webp", "heic", "heif", "avif", "jxr"];

/// The lines of text in a picture, as Windows reads them (turned as its EXIF says, made smaller first if
/// it's bigger than OCR takes).
pub fn lines(path: &str) -> Result<Vec<String>, String> {
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, BitmapTransform, ColorManagementMode, ExifOrientationMode};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::{FileAccessMode, StorageFile};
    let path = HSTRING::from(path);
    let read = std::thread::scope(|scope| {
        scope
            .spawn(|| -> windows::core::Result<Option<Vec<String>>> {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                }
                let Ok(engine) = OcrEngine::TryCreateFromUserProfileLanguages() else { return Ok(None) };
                let file = StorageFile::GetFileFromPathAsync(&path)?.join()?;
                let stream = file.OpenAsync(FileAccessMode::Read)?.join()?;
                let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
                let (width, height) = (decoder.PixelWidth()?, decoder.PixelHeight()?);
                let largest = OcrEngine::MaxImageDimension()?.max(1);
                let transform = BitmapTransform::new()?;
                if width.max(height) > largest {
                    let scale = largest as f64 / width.max(height) as f64;
                    transform.SetScaledWidth(((width as f64 * scale) as u32).max(1))?;
                    transform.SetScaledHeight(((height as f64 * scale) as u32).max(1))?;
                }
                let bitmap = decoder
                    .GetSoftwareBitmapTransformedAsync(BitmapPixelFormat::Bgra8, BitmapAlphaMode::Premultiplied, &transform, ExifOrientationMode::RespectExifOrientation, ColorManagementMode::DoNotColorManage)?
                    .join()?;
                let result = engine.RecognizeAsync(&bitmap)?.join()?;
                Ok(Some(result.Lines()?.into_iter().filter_map(|line| line.Text().ok()).map(|text| text.to_string()).filter(|text| !text.trim().is_empty()).collect()))
            })
            .join()
            .unwrap_or_else(|_| Err(E_FAIL.into()))
    });
    match read {
        Ok(Some(lines)) => Ok(lines),
        Ok(None) => Err("Windows has no OCR language installed: Settings → Time & language → Language adds one with a language".into()),
        Err(e) => Err(format!("Windows couldn't read this picture: {}", e.message())),
    }
}

/// A picture's text, a line each, for Copy text.
#[tauri::command]
pub async fn ocr_text(path: String) -> Result<Vec<String>, String> {
    crate::fsx::blocking(move || lines(&path)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG with text drawn on it by Windows (GDI+ through PowerShell), read back. Skipped where
    /// Windows has no OCR language.
    #[test]
    fn reads_text_windows_draws() {
        let dir = std::env::temp_dir().join(format!("glint-ocr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("text.png");
        let script = format!(
            "Add-Type -AssemblyName System.Drawing; $b = New-Object System.Drawing.Bitmap 900, 220; $g = [System.Drawing.Graphics]::FromImage($b); $g.Clear([System.Drawing.Color]::White); \
             $g.DrawString('Glint reads pictures', (New-Object System.Drawing.Font 'Segoe UI', 40), [System.Drawing.Brushes]::Black, 20, 20); \
             $g.DrawString('Invoice 2026-10-24', (New-Object System.Drawing.Font 'Segoe UI', 40), [System.Drawing.Brushes]::Black, 20, 110); $b.Save('{}')",
            png.display()
        );
        let made = std::process::Command::new("powershell").args(["-NoProfile", "-Command", &script]).status();
        if !made.is_ok_and(|s| s.success()) {
            return;
        }
        match lines(png.to_str().unwrap()) {
            Ok(lines) => assert_eq!(lines, ["Glint reads pictures", "Invoice 2026-10-24"]),
            Err(e) if e.contains("no OCR language") => eprintln!("skipped: {e}"),
            Err(e) => panic!("{e}"),
        }
        assert!(lines(dir.join("missing.png").to_str().unwrap()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
