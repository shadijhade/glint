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

/// A scanned PDF's text (one with no text of its own): its first `max_pages` pages drawn by Windows' PDF
/// renderer at the size OCR reads best, each read by Windows' OCR, as (label "Page n", the page's lines).
pub fn pdf_pages(path: &str, max_pages: u32) -> Result<Vec<(String, String)>, String> {
    use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::StorageFile;
    use windows::Storage::Streams::InMemoryRandomAccessStream;
    let path = HSTRING::from(path);
    let read = std::thread::scope(|scope| {
        scope
            .spawn(|| -> windows::core::Result<Option<Vec<(String, String)>>> {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                }
                let Ok(engine) = OcrEngine::TryCreateFromUserProfileLanguages() else { return Ok(None) };
                let largest = OcrEngine::MaxImageDimension()?.clamp(1, 2600) as f32;
                let file = StorageFile::GetFileFromPathAsync(&path)?.join()?;
                let document = PdfDocument::LoadFromFileAsync(&file)?.join()?;
                let mut pages = Vec::new();
                for n in 0..document.PageCount()?.min(max_pages) {
                    let page = document.GetPage(n)?;
                    let size = page.Size()?;
                    let scale = largest / size.Width.max(size.Height).max(1.0);
                    let options = PdfPageRenderOptions::new()?;
                    options.SetDestinationWidth(((size.Width * scale).round() as u32).max(1))?;
                    options.SetDestinationHeight(((size.Height * scale).round() as u32).max(1))?;
                    let stream = InMemoryRandomAccessStream::new()?;
                    page.RenderWithOptionsToStreamAsync(&stream, &options)?.join()?;
                    let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
                    let bitmap = decoder.GetSoftwareBitmapAsync()?.join()?;
                    let bitmap = windows::Graphics::Imaging::SoftwareBitmap::ConvertWithAlpha(&bitmap, BitmapPixelFormat::Bgra8, BitmapAlphaMode::Premultiplied)?;
                    let result = engine.RecognizeAsync(&bitmap)?.join()?;
                    let lines: Vec<String> = result.Lines()?.into_iter().filter_map(|l| l.Text().ok()).map(|t| t.to_string()).filter(|t| !t.trim().is_empty()).collect();
                    if !lines.is_empty() {
                        pages.push((format!("Page {}", n + 1), lines.join("\n")));
                    }
                }
                Ok(Some(pages))
            })
            .join()
            .unwrap_or_else(|_| Err(E_FAIL.into()))
    });
    match read {
        Ok(Some(pages)) => Ok(pages),
        Ok(None) => Err("Windows has no OCR language installed".into()),
        Err(e) => Err(format!("Windows couldn't read this PDF: {}", e.message())),
    }
}

/// A picture's text, a line each, for Copy text.
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

    /// A "scanned" PDF (a JPEG of text on a page, nothing else) made here, read page by page.
    #[test]
    fn reads_scanned_pdfs() {
        let dir = std::env::temp_dir().join(format!("glint-ocr-pdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let jpg = dir.join("scan.jpg");
        let script = format!(
            "Add-Type -AssemblyName System.Drawing; $b = New-Object System.Drawing.Bitmap 1240, 400; $g = [System.Drawing.Graphics]::FromImage($b); $g.Clear([System.Drawing.Color]::White); \
             $g.DrawString('Scanned contract 2026', (New-Object System.Drawing.Font 'Segoe UI', 48), [System.Drawing.Brushes]::Black, 30, 60); $b.Save('{}', [System.Drawing.Imaging.ImageFormat]::Jpeg)",
            jpg.display()
        );
        let made = std::process::Command::new("powershell").args(["-NoProfile", "-Command", &script]).status();
        if !made.is_ok_and(|s| s.success()) {
            return;
        }
        let image = std::fs::read(&jpg).unwrap();
        let pdf = dir.join("scan.pdf");
        std::fs::write(&pdf, pdf_of_jpeg(&image, 1240, 400)).unwrap();
        match pdf_pages(pdf.to_str().unwrap(), 5) {
            Ok(pages) => assert_eq!(pages, [("Page 1".to_string(), "Scanned contract 2026".to_string())]),
            Err(e) if e.contains("no OCR language") => eprintln!("skipped: {e}"),
            Err(e) => panic!("{e}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A one-page PDF that's only the JPEG, at 72 dpi.
    fn pdf_of_jpeg(jpeg: &[u8], width: u32, height: u32) -> Vec<u8> {
        let content = format!("q {width} 0 0 {height} 0 0 cm /Im0 Do Q");
        let objects: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>").into_bytes(),
            [format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(), content.into_bytes(), b"\nendstream".to_vec()].concat(),
            [format!("<< /Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n", jpeg.len()).into_bytes(), jpeg.to_vec(), b"\nendstream".to_vec()].concat(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n", i + 1).into_bytes());
            out.extend(object);
            out.extend(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).into_bytes());
        for offset in offsets {
            out.extend(format!("{offset:010} 00000 n \n").into_bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).into_bytes());
        out
    }
}
