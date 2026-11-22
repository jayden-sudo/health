//! File → items with candidates: an image and the same page as a PDF. Needs the models
//! (HEALTH_OCR_MODELS=<dir with ppocrv5_* and e5_*>); the input checks run without.

use health_ocr::input::{InputError, read_input};
use health_ocr::{Models, embed::Embedder, engine::Engine, recognize};

fn fx(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

#[test]
fn files_that_are_not_images_or_pdfs_are_refused_with_a_reason() {
    assert_eq!(read_input(b"hello, not an image").err(), Some(InputError::Unsupported));
    assert_eq!(read_input(&vec![0u8; 41 * 1024 * 1024]).err(), Some(InputError::TooBig));
    assert_eq!(read_input(b"%PDF-1.7 broken").err(), Some(InputError::Unreadable));
    let input = read_input(&fx("cbc.png")).unwrap();
    assert_eq!(input.sha.len(), 64);
    assert_eq!((input.pages[0].width, input.pages[0].height), (2480, 1750));
}

#[test]
fn an_image_and_the_same_page_as_pdf_give_the_report() {
    let Some(dir) = std::env::var_os("HEALTH_OCR_MODELS").map(std::path::PathBuf::from) else {
        eprintln!("skipped: set HEALTH_OCR_MODELS");
        return;
    };
    let r = |f: &str| std::fs::read(dir.join(f)).unwrap();
    let m = Models {
        engine: Engine::new(&r("ppocrv5_det.onnx"), &r("ppocrv5_rec.onnx"), &String::from_utf8(r("ppocrv5_dict.txt")).unwrap()).unwrap(),
        embed: Some(Embedder::new(&r("e5_model_int8.onnx"), &r("e5_tokenizer.json")).unwrap()),
    };
    let png = recognize(&m, &read_input(&fx("cbc.png")).unwrap(), 2026, &mut |_, _| {}).unwrap();
    assert_eq!(png.items.len(), 14);
    assert_eq!(png.date.as_deref(), Some("2026-09-27"));
    assert!(png.items.iter().all(|i| i.pre.is_some()), "every CBC line has a clear measure");
    assert!(png.text.contains("Hemoglobin"));

    let pdf_in = read_input(&fx("cbc.pdf")).unwrap();
    assert_eq!(pdf_in.pages.len(), 1);
    let pdf = recognize(&m, &pdf_in, 2026, &mut |_, _| {}).unwrap();
    let key = |r: &health_core::ocr::Recognized| -> Vec<(String, String)> {
        r.items.iter().map(|i| (i.item.name.clone(), serde_json::to_string(&i.item.v).unwrap())).collect()
    };
    let (a, b) = (key(&png), key(&pdf));
    let same = a.iter().filter(|x| b.contains(x)).count();
    assert!(same >= 13, "PDF reads the page too: {same}/14 same items\npng {a:?}\npdf {b:?}");
    assert_eq!(pdf.date.as_deref(), Some("2026-09-27"));
}

#[test]
fn a_small_file_declaring_a_huge_image_is_too_big() {
    // a valid PNG header for 60000×60000 pixels: refused before any pixel is decoded
    let mut png = Vec::new();
    let img = image::RgbaImage::new(1, 1);
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    // patch width and height in the IHDR chunk (bytes 16..24, big endian)
    png[16..20].copy_from_slice(&60_000u32.to_be_bytes());
    png[20..24].copy_from_slice(&60_000u32.to_be_bytes());
    let crc = crc32fast::hash(&png[12..29]); // the chunk's CRC covers its type and data
    png[29..33].copy_from_slice(&crc.to_be_bytes());
    assert_eq!(read_input(&png).err(), Some(InputError::TooBig));
}
