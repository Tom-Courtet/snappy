//! Moteur macOS : framework Vision (hors ligne, aucune autorisation pour analyser
//! une image déjà capturée).
//!
//! - texte : `VNRecognizeTextRequest` (précis, correction linguistique, français et
//!   anglais, détection automatique de la langue) ; boîte d'un passage par
//!   `-[VNRecognizedText boundingBoxForRange:error:]` ;
//! - codes : `VNDetectBarcodesRequest` (toutes les symbologies) ;
//! - détourage : `VNGenerateForegroundInstanceMaskRequest` (macOS 14).

use std::ops::Range;

use anyhow::{Context, Result, anyhow, bail, ensure};
use image::RgbaImage;
use objc2::encode::{Encode, Encoding};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject, NSObjectProtocol};
use objc2::{AnyThread, msg_send, sel};
use objc2_core_foundation::{CFRetained, CGRect};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
    kCVPixelFormatType_OneComponent8, kCVPixelFormatType_OneComponent32Float, kCVReturnSuccess,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSString};
use objc2_vision::{
    VNDetectBarcodesRequest, VNGenerateForegroundInstanceMaskRequest, VNImageOption,
    VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedText, VNRectangleObservation,
    VNRequest, VNRequestTextRecognitionLevel,
};

use super::codes::Code;
use super::geom::{self, Rect};
use super::{Recognized, TextLine};

/// Langues demandées à Vision, par ordre de préférence.
const LANGUAGES: [&str; 2] = ["fr-FR", "en-US"];

/// Moteur et langues, pour la fenêtre de résultat.
pub fn engine_info() -> String {
    "Vision (macOS) · français, anglais · détection automatique de la langue".into()
}

/// Texte de l'image, ligne par ligne, avec la boîte de chaque passage demandé par
/// `spans_of` (indices d'octets dans le texte de la ligne).
pub fn recognize<T>(
    img: &RgbaImage,
    spans_of: impl Fn(&str) -> Vec<(T, Range<usize>)>,
) -> Result<Vec<Recognized<T>>> {
    let (w, h) = img.dimensions();
    let cg = cg_image(img, true)?;
    autoreleasepool(|_| {
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(true);
        let languages = NSArray::from_retained_slice(&LANGUAGES.map(NSString::from_str));
        request.setRecognitionLanguages(&languages);
        // macOS 13 et plus récent.
        if request.respondsToSelector(sel!(setAutomaticallyDetectsLanguage:)) {
            request.setAutomaticallyDetectsLanguage(true);
        }
        perform(&handler(&cg), &request)?;
        let Some(results) = request.results() else {
            return Ok(Vec::new());
        };
        let mut lines = Vec::new();
        for observation in results.iter() {
            let Some(top) = observation.topCandidates(1).firstObject() else {
                continue;
            };
            let text = top.string().to_string();
            // SAFETY : propriété en lecture seule d'une observation de Vision.
            let rect = to_px(unsafe { observation.boundingBox() }, w, h);
            let spans = spans_of(&text)
                .into_iter()
                .map(|(tag, span)| {
                    let r = span_box(&top, &text, span.clone(), w, h)
                        .unwrap_or_else(|| geom::split_line(rect, &text, span));
                    (tag, r)
                })
                .collect();
            lines.push(Recognized {
                line: TextLine {
                    text,
                    rect,
                    confidence: top.confidence(),
                },
                spans,
            });
        }
        Ok(lines)
    })
}

/// Codes-barres et QR codes repérés par Vision.
pub fn barcodes(img: &RgbaImage) -> Result<Vec<Code>> {
    let (w, h) = img.dimensions();
    let cg = cg_image(img, true)?;
    autoreleasepool(|_| {
        // SAFETY : constructeur sans argument.
        let request = unsafe { VNDetectBarcodesRequest::new() };
        perform(&handler(&cg), &request)?;
        // SAFETY : lecture des résultats une fois la requête terminée.
        let Some(results) = (unsafe { request.results() }) else {
            return Ok(Vec::new());
        };
        let mut codes = Vec::new();
        for observation in results.iter() {
            // SAFETY : propriétés en lecture seule.
            let (payload, symbology, bbox) = unsafe {
                (
                    observation.payloadStringValue(),
                    observation.symbology(),
                    observation.boundingBox(),
                )
            };
            let Some(text) = payload.map(|p| p.to_string()) else {
                continue;
            };
            let symbology = symbology.to_string();
            let kind = symbology
                .strip_prefix("VNBarcodeSymbology")
                .unwrap_or(&symbology)
                .to_owned();
            super::codes::push_unique(
                &mut codes,
                Code {
                    text,
                    kind,
                    rect: to_px(bbox, w, h),
                },
            );
        }
        Ok(codes)
    })
}

/// Sujet au premier plan gardé, arrière-plan rendu transparent (macOS 14).
pub fn remove_background(img: &RgbaImage) -> Result<RgbaImage> {
    ensure!(
        AnyClass::get(c"VNGenerateForegroundInstanceMaskRequest").is_some(),
        "la suppression de l'arrière-plan demande macOS 14 ou plus récent"
    );
    let cg = cg_image(img, false)?;
    autoreleasepool(|_| {
        // SAFETY : constructeur sans argument ; classe présente (vérifié).
        let request = unsafe { VNGenerateForegroundInstanceMaskRequest::new() };
        let handler = handler(&cg);
        perform(&handler, &request)?;
        // SAFETY : lecture des résultats une fois la requête terminée.
        let observation = unsafe { request.results() }
            .and_then(|r| r.firstObject())
            .context("aucun sujet détecté au premier plan")?;
        // SAFETY : masque des sujets trouvés par cette requête, à la taille de
        // l'image du même `handler`.
        let mask = unsafe {
            let instances = observation.allInstances();
            observation.generateScaledMaskForImageForInstances_fromRequestHandler_error(
                &instances, &handler,
            )
        }
        .map_err(|e| vision_error("masque du premier plan", &e))?;
        apply_mask(img, &mask)
    })
}

/// Le masque (0 : arrière-plan, 1 : sujet) multiplié dans le canal alpha.
fn apply_mask(img: &RgbaImage, mask: &CVPixelBuffer) -> Result<RgbaImage> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let (mw, mh) = (CVPixelBufferGetWidth(mask), CVPixelBufferGetHeight(mask));
    ensure!(
        (mw, mh) == (w, h),
        "masque de {mw}×{mh} pour une image de {w}×{h}"
    );
    let format = CVPixelBufferGetPixelFormatType(mask);
    let bytes_per_value = if format == kCVPixelFormatType_OneComponent32Float {
        4
    } else if format == kCVPixelFormatType_OneComponent8 {
        1
    } else {
        bail!("format de masque inattendu : {format:#x}")
    };
    // SAFETY : verrou en lecture, levé plus bas quoi qu'il arrive.
    let locked = unsafe { CVPixelBufferLockBaseAddress(mask, CVPixelBufferLockFlags::ReadOnly) };
    ensure!(locked == kCVReturnSuccess, "masque illisible ({locked})");
    let base = CVPixelBufferGetBaseAddress(mask).cast::<u8>().cast_const();
    let stride = CVPixelBufferGetBytesPerRow(mask);
    let mut out = img.clone();
    if !base.is_null() && stride >= w * bytes_per_value {
        for (x, y, px) in out.enumerate_pixels_mut() {
            let offset = y as usize * stride + x as usize * bytes_per_value;
            // SAFETY : `offset` reste dans la ligne `y` (stride ≥ largeur × taille
            // d'une valeur) du tampon de `h` lignes, verrouillé.
            let value = unsafe {
                if bytes_per_value == 4 {
                    base.add(offset).cast::<f32>().read_unaligned()
                } else {
                    f32::from(*base.add(offset)) / 255.0
                }
            };
            px.0[3] = (f32::from(px.0[3]) * value.clamp(0.0, 1.0)).round() as u8;
        }
    }
    // SAFETY : même tampon, mêmes options que le verrou.
    unsafe { CVPixelBufferUnlockBaseAddress(mask, CVPixelBufferLockFlags::ReadOnly) };
    ensure!(!base.is_null(), "masque vide");
    Ok(out)
}

/// `NSRange`, défini ici pour ne pas changer les fonctions d'`objc2-foundation`.
#[repr(C)]
#[derive(Clone, Copy)]
struct NsRange {
    location: usize,
    length: usize,
}

// SAFETY : même disposition et même encodage que `NSRange` (deux `NSUInteger`).
unsafe impl Encode for NsRange {
    const ENCODING: Encoding = Encoding::Struct("_NSRange", &[usize::ENCODING, usize::ENCODING]);
}

/// Boîte du passage `span` (octets de `text`) selon Vision. Vision rend la boîte
/// des mots entiers (« Contact:jean@exemple.fr » d'un bloc) : la part du passage
/// dans son mot est prise au prorata des caractères.
fn span_box(
    top: &VNRecognizedText,
    text: &str,
    span: Range<usize>,
    w: u32,
    h: u32,
) -> Option<Rect> {
    let start = geom::utf16_offset(text, span.start);
    let range = NsRange {
        location: start,
        length: geom::utf16_offset(text, span.end) - start,
    };
    // SAFETY : méthode publique de `VNRecognizedText` (macOS 10.15), plage en unités
    // UTF-16 dans les bornes de la chaîne reconnue.
    let result: Result<Retained<VNRectangleObservation>, Retained<NSError>> =
        unsafe { msg_send![top, boundingBoxForRange: range, error: _] };
    // SAFETY : propriété en lecture seule.
    let word_box = to_px(unsafe { result.ok()?.boundingBox() }, w, h);
    let word = geom::word_around(text, &span);
    Some(geom::split_line(
        word_box,
        &text[word.clone()],
        span.start - word.start..span.end - word.start,
    ))
}

fn to_px(r: CGRect, w: u32, h: u32) -> Rect {
    geom::normalized_to_px(r.origin.x, r.origin.y, r.size.width, r.size.height, w, h)
}

fn handler(cg: &CGImage) -> Retained<VNImageRequestHandler> {
    let options = NSDictionary::<VNImageOption, AnyObject>::new();
    // SAFETY : dictionaire d'options vide, image valide.
    unsafe {
        VNImageRequestHandler::initWithCGImage_options(VNImageRequestHandler::alloc(), cg, &options)
    }
}

fn perform(handler: &VNImageRequestHandler, request: &VNRequest) -> Result<()> {
    handler
        .performRequests_error(&NSArray::from_slice(&[request]))
        .map_err(|e| vision_error("analyse", &e))
}

fn vision_error(what: &str, e: &NSError) -> anyhow::Error {
    anyhow!("Vision, {what} : {}", e.localizedDescription())
}

/// Copie de l'image pour CoreGraphics (sRGB, alpha prémultiplié). `flatten` : posée
/// sur du blanc (texte et codes : un fond transparent ne doit pas devenir noir).
fn cg_image(img: &RgbaImage, flatten: bool) -> Result<CFRetained<CGImage>> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    ensure!(w > 0 && h > 0, "image vide");
    let mut data = img.as_raw().clone();
    for px in data.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        if a == 255 {
            continue;
        }
        for c in &mut px[..3] {
            let v = u32::from(*c) * a + if flatten { 255 * (255 - a) } else { 0 };
            *c = ((v + 127) / 255) as u8;
        }
        if flatten {
            px[3] = 255;
        }
    }
    // SAFETY : constante fournie par CoreGraphics.
    let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
        .context("espace sRGB indisponible")?;
    let info = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
    // SAFETY : `data` fait `w * 4 * h` octets et survit au contexte ; l'image créée
    // garde sa propre copie des pixels.
    let ctx = unsafe {
        CGBitmapContextCreate(data.as_mut_ptr().cast(), w, h, 8, w * 4, Some(&srgb), info)
    }
    .context("création du contexte bitmap")?;
    CGBitmapContextCreateImage(Some(&ctx)).context("création de l'image CoreGraphics")
}
