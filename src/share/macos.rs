//! Partage, courriel et impression avec AppKit.
//!
//! - Feuille de partage : `NSSharingServicePicker` (AirDrop, Mail, Messages, Notes…).
//!   En ligne de commande, une fenêtre sans bord de 2 points sous la souris lui sert
//!   d'ancre ; le processus s'arrête quand le partage est fait ou annulé.
//! - Courriel : le service `NSSharingServiceNameComposeEmail` (client par défaut).
//! - Impression : `NSPrintOperation` sur une vue (image + nom de fichier en option),
//!   avec le panneau du système (aperçu, imprimante, copies, échelle).

use std::ffi::c_void;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use image::RgbaImage;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSEvent, NSFont,
    NSImage, NSImageAlignment, NSImageScaling, NSImageView, NSPaperOrientation,
    NSPopUpMenuWindowLevel, NSPrintInfo, NSPrintOperation, NSPrintingPaginationMode,
    NSSharingService, NSSharingServiceDelegate, NSSharingServiceNameComposeEmail,
    NSSharingServicePicker, NSSharingServicePickerDelegate, NSTextAlignment, NSTextField, NSView,
    NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSData, NSError, NSPoint, NSRect, NSRectEdge, NSSize, NSString, NSURL,
};

use super::PrintOptions;

/// Durée maximale d'un partage lancé en ligne de commande : filet de sécurité si un
/// service ne prévient jamais de la fin.
const WATCHDOG: Duration = Duration::from_secs(30 * 60);

fn main_thread() -> Result<MainThreadMarker> {
    MainThreadMarker::new().context("AppKit : pas sur le thread principal")
}

/// Les fichiers en `NSURL`, dans un `NSArray` d'objets.
fn file_urls(paths: &[PathBuf]) -> Retained<NSArray> {
    let urls: Vec<Retained<AnyObject>> = paths
        .iter()
        .map(|p| {
            let abs = std::path::absolute(p).unwrap_or_else(|_| p.clone());
            let url = NSURL::fileURLWithPath(&NSString::from_str(&abs.to_string_lossy()));
            Retained::into_super(Retained::into_super(url))
        })
        .collect();
    NSArray::from_retained_slice(&urls)
}

/// Fin du processus de partage (ligne de commande seulement).
fn finish(code: i32) -> ! {
    std::process::exit(code)
}

define_class!(
    // SAFETY : NSObject n'impose rien aux sous-classes ; pas de `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SnappyShareDelegate"]
    struct ShareDelegate;

    unsafe impl NSObjectProtocol for ShareDelegate {}

    unsafe impl NSSharingServicePickerDelegate for ShareDelegate {
        #[unsafe(method_id(sharingServicePicker:delegateForSharingService:))]
        fn delegate_for_service(
            &self,
            _picker: &NSSharingServicePicker,
            _service: &NSSharingService,
        ) -> Option<Retained<ProtocolObject<dyn NSSharingServiceDelegate>>> {
            Some(ProtocolObject::from_retained(self.retain()))
        }

        #[unsafe(method(sharingServicePicker:didChooseSharingService:))]
        fn did_choose(&self, _picker: &NSSharingServicePicker, service: Option<&NSSharingService>) {
            if service.is_none() {
                // Feuille fermée sans choisir.
                finish(0);
            }
        }
    }

    unsafe impl NSSharingServiceDelegate for ShareDelegate {
        #[unsafe(method(sharingService:didShareItems:))]
        fn did_share(&self, _service: &NSSharingService, _items: &NSArray) {
            finish(0);
        }

        #[unsafe(method(sharingService:didFailToShareItems:error:))]
        fn did_fail(&self, _service: &NSSharingService, _items: &NSArray, error: &NSError) {
            let message = error.localizedDescription().to_string();
            // Annulation par l'utilisateur : pas une erreur à signaler.
            if error.code() != 3072 {
                eprintln!("partage impossible : {message}");
                finish(1);
            }
            finish(0);
        }
    }
);

impl ShareDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY : `init` de NSObject.
        unsafe { msg_send![super(this), init] }
    }
}

/// Application sans icône dans le Dock, au premier plan.
fn app(mtm: MainThreadMarker) -> Retained<NSApplication> {
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.finishLaunching();
    app.activate();
    app
}

/// Fenêtre transparente de 2 points sous la souris : l'ancre de la feuille.
fn anchor_window(mtm: MainThreadMarker) -> Retained<NSWindow> {
    let mouse = NSEvent::mouseLocation();
    let frame = NSRect::new(
        NSPoint::new(mouse.x - 1.0, mouse.y - 1.0),
        NSSize::new(2.0, 2.0),
    );
    // SAFETY : paramètres valides ; la fenêtre n'est pas libérée à la fermeture
    // (on la garde avec `Retained`).
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.setLevel(NSPopUpMenuWindowLevel);
    window.makeKeyAndOrderFront(None);
    window
}

fn watchdog() {
    std::thread::spawn(|| {
        std::thread::sleep(WATCHDOG);
        finish(0);
    });
}

/// `snappy --share` : la feuille de partage sous la souris, jusqu'à la fin du partage.
pub fn run_picker(paths: &[PathBuf]) -> Result<()> {
    let mtm = main_thread()?;
    let app = app(mtm);
    let window = anchor_window(mtm);
    let view = window
        .contentView()
        .context("vue de la fenêtre d'ancrage")?;
    let delegate = ShareDelegate::new(mtm);
    // SAFETY : des NSURL de fichiers, que la feuille sait partager.
    let picker = unsafe {
        NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &file_urls(paths))
    };
    picker.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    picker.showRelativeToRect_ofView_preferredEdge(view.bounds(), &view, NSRectEdge::MinY);
    watchdog();
    app.run();
    Ok(())
}

/// `snappy --mail` : nouveau message avec les pièces jointes.
pub fn run_mail(paths: &[PathBuf]) -> Result<()> {
    let mtm = main_thread()?;
    let app = app(mtm);
    // SAFETY : nom de service fourni par AppKit.
    let service =
        NSSharingService::sharingServiceNamed(unsafe { NSSharingServiceNameComposeEmail })
            .context("service de courriel introuvable")?;
    let items = file_urls(paths);
    // SAFETY : des NSURL de fichiers.
    if !unsafe { service.canPerformWithItems(Some(&items)) } {
        bail!(
            "aucun client de messagerie ne peut envoyer ces fichiers (configure un compte \
             dans Mail, ou choisis un client par défaut)"
        );
    }
    let delegate = ShareDelegate::new(mtm);
    service.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    // SAFETY : idem.
    unsafe { service.performWithItems(&items) };
    watchdog();
    app.run();
    Ok(())
}

thread_local! {
    /// Dernière feuille ouverte dans une fenêtre egui, gardée en vie pendant
    /// qu'elle est affichée.
    static PICKER: std::cell::RefCell<Option<Retained<NSSharingServicePicker>>> =
        const { std::cell::RefCell::new(None) };
}

/// Feuille de partage accrochée à `rect` (origine en haut à gauche) dans `view`.
///
/// # Safety
/// `view` doit pointer vers une `NSView` vivante.
pub unsafe fn picker_in_view(view: *mut c_void, rect: [f64; 4], paths: &[PathBuf]) -> Result<()> {
    main_thread()?;
    // SAFETY : garanti par l'appelant.
    let view: &NSView = unsafe { &*view.cast::<NSView>() };
    let [x, y, w, h] = rect;
    let y = if view.isFlipped() {
        y
    } else {
        view.bounds().size.height - y - h
    };
    let rect = NSRect::new(NSPoint::new(x, y), NSSize::new(w.max(1.0), h.max(1.0)));
    // SAFETY : des NSURL de fichiers.
    let picker = unsafe {
        NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &file_urls(paths))
    };
    picker.showRelativeToRect_ofView_preferredEdge(rect, view, NSRectEdge::MinY);
    PICKER.with(|p| *p.borrow_mut() = Some(picker));
    Ok(())
}

/// Hauteur réservée au nom du fichier, en points.
const NAME_HEIGHT: f64 = 16.0;

/// Imprime l'image avec le panneau d'impression du système (aperçu compris).
pub fn print_image(img: &RgbaImage, name: &str, options: PrintOptions) -> Result<()> {
    let mtm = main_thread()?;
    let app = app(mtm);
    let png = super::docgen::png_bytes(img)?;
    let data = NSData::with_bytes(&png);
    let image = NSImage::initWithData(NSImage::alloc(), &data).context("image illisible")?;

    let info = NSPrintInfo::sharedPrintInfo();
    info.setOrientation(if options.orientation.landscape(img.dimensions()) {
        NSPaperOrientation::Landscape
    } else {
        NSPaperOrientation::Portrait
    });
    info.setHorizontalPagination(NSPrintingPaginationMode::Fit);
    info.setVerticalPagination(NSPrintingPaginationMode::Fit);
    info.setHorizontallyCentered(true);
    info.setVerticallyCentered(true);
    let paper = info.paperSize();
    let width = (paper.width - info.leftMargin() - info.rightMargin()).max(72.0);
    let height = (paper.height - info.topMargin() - info.bottomMargin()).max(72.0);

    let page = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height)),
    );
    let label_h = if options.file_name { NAME_HEIGHT } else { 0.0 };
    let view = NSImageView::initWithFrame(
        NSImageView::alloc(mtm),
        NSRect::new(
            NSPoint::new(0.0, label_h),
            NSSize::new(width, height - label_h),
        ),
    );
    view.setImage(Some(&image));
    view.setImageAlignment(NSImageAlignment::AlignCenter);
    view.setImageScaling(if options.fit_to_page {
        NSImageScaling::ScaleProportionallyUpOrDown
    } else {
        NSImageScaling::ScaleProportionallyDown
    });
    page.addSubview(&view);
    if options.file_name {
        // Nom du fichier en bas à droite (§ 13.1).
        let label = NSTextField::labelWithString(&NSString::from_str(name), mtm);
        label.setAlignment(NSTextAlignment::Right);
        label.setFont(Some(&NSFont::systemFontOfSize(9.0)));
        label.setTextColor(Some(&NSColor::darkGrayColor()));
        label.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(width, NAME_HEIGHT - 4.0),
        ));
        page.addSubview(&label);
    }

    let op = NSPrintOperation::printOperationWithView_printInfo(&page, &info);
    op.setJobTitle(Some(&NSString::from_str(name)));
    op.setShowsPrintPanel(true);
    op.setShowsProgressPanel(true);
    // Panneau modal : rend la main une fois imprimé ou annulé.
    op.runOperation();
    drop(app);
    Ok(())
}
