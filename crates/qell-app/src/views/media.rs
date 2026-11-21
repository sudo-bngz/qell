//! The tab that shows a picture, a video or a sound file.
//!
//! A picture is decoded and drawn, fitted to the window or at its own
//! size. A video shows a frame with a slider to move through it, and a
//! sound file shows what it is; neither plays here — `o` hands the file
//! to whatever you normally open it with.

use egui::{Align2, Color32, Key, Rect, TextureOptions, Vec2};

use crate::app::QellApp;
use crate::document::{Document, MediaTab};
use crate::preview::{self, Media};

/// Read the file the first time the tab is drawn, and again when the
/// slider has been moved. Decoding happens here rather than on a worker
/// because it is a file you just asked to see: a picture is milliseconds,
/// and a frame is one `ffmpeg` seek.
fn load(doc: &mut Document, ctx: &egui::Context) {
    let Some(path) = doc.buffer.path().map(std::path::Path::to_path_buf) else { return };
    let Some(media) = &mut doc.media else { return };
    let name = path.to_string_lossy().into_owned();
    match media.kind {
        Media::Image => match preview::read_image(&path) {
            Ok(picture) => put(media, ctx, &name, picture),
            Err(e) => media.error = Some(e),
        },
        Media::Video => {
            if media.info.is_none() {
                match preview::probe(&path) {
                    Ok(info) => media.info = Some(info),
                    Err(e) => {
                        media.error = Some(e.to_string());
                        media.pending = false;
                        return;
                    }
                }
            }
            let at = media.wanted;
            match preview::frame(&path, at) {
                Ok(picture) => {
                    put(media, ctx, &format!("{name}@{at}"), picture);
                    media.at = at;
                }
                Err(e) => media.error = Some(e.to_string()),
            }
        }
        Media::Audio => match preview::probe(&path) {
            Ok(info) => media.info = Some(info),
            Err(e) => media.error = Some(e.to_string()),
        },
    }
    media.pending = false;
}

fn put(media: &mut MediaTab, ctx: &egui::Context, name: &str, picture: preview::Picture) {
    let image = egui::ColorImage::from_rgba_unmultiplied([picture.width, picture.height], &picture.rgba);
    media.pixels = (picture.width, picture.height);
    media.texture = Some(ctx.load_texture(name, image, TextureOptions::LINEAR));
    media.error = None;
}

/// Draw the tab. Returns false when this document isn't a media one.
pub fn show(app: &mut QellApp, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
    let Some(doc) = app.docs.get(app.active) else { return false };
    if doc.media.is_none() {
        return false;
    }
    if app.docs[app.active].media.as_ref().is_some_and(|m| m.pending) {
        let doc = &mut app.docs[app.active];
        load(doc, ctx);
    }
    keys(app, ctx);

    let (dim, text, accent, error) = (app.palette.text_dim, app.palette.text, app.palette.accent, app.palette.error);
    let font = egui::FontId::monospace(app.config.font_size.max(11.0) - 1.0);
    let Some(doc) = app.docs.get(app.active) else { return true };
    let Some(media) = &doc.media else { return true };
    let name = doc.buffer.path().map(|p| p.display().to_string()).unwrap_or_default();

    // What it is, along the top.
    let mut line = vec![media.kind.label().to_string(), preview::bytes(media.size)];
    if media.pixels != (0, 0) {
        line.push(format!("{}×{}", media.pixels.0, media.pixels.1));
    } else if let Some(info) = &media.info {
        if let (Some(w), Some(h)) = (info.width, info.height) {
            line.push(format!("{w}×{h}"));
        }
    }
    if let Some(info) = &media.info {
        if let Some(d) = info.duration() {
            line.push(d);
        }
        if let Some(fps) = info.fps {
            line.push(format!("{fps:.0} fps"));
        }
        for codec in [&info.video_codec, &info.audio_codec].into_iter().flatten() {
            line.push(codec.clone());
        }
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.label(egui::RichText::new(line.join("   ")).font(font.clone()).color(dim));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(10.0);
            if ui
                .add(egui::Button::new(egui::RichText::new("Open externally  (o)").font(font.clone()).color(dim)).frame(false))
                .clicked()
            {
                open_externally(app, &name);
            }
        });
    });
    ui.add_space(4.0);

    let Some(doc) = app.docs.get(app.active) else { return true };
    let Some(media) = &doc.media else { return true };
    let area = ui.available_rect_before_wrap();
    // A video keeps a strip at the bottom for the slider.
    let slider_h = if media.kind == Media::Video && media.info.is_some() { 34.0 } else { 0.0 };
    let canvas = Rect::from_min_max(area.min, egui::Pos2::new(area.max.x, area.max.y - slider_h));
    let painter = ui.painter_at(canvas);

    match (&media.texture, &media.error, media.pending) {
        (_, _, true) => {
            painter.text(canvas.center(), Align2::CENTER_CENTER, "reading…", font.clone(), dim);
        }
        (_, Some(message), _) => {
            // Wrapped: the ffmpeg hint is a sentence, not a word.
            let galley = painter.layout(message.clone(), font.clone(), error, canvas.width().min(560.0));
            let at = canvas.center() - galley.size() / 2.0;
            painter.galley(at, galley, error);
        }
        (Some(texture), _, _) => {
            let size = Vec2::new(media.pixels.0 as f32, media.pixels.1 as f32);
            let scale = if media.actual_size { 1.0 } else { (canvas.width() / size.x).min(canvas.height() / size.y).min(1.0) };
            let drawn = size * scale;
            let at = Rect::from_center_size(canvas.center(), drawn);
            painter.image(texture.id(), at, Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), Color32::WHITE);
            if media.actual_size && (drawn.x > canvas.width() || drawn.y > canvas.height()) {
                let hint = "f fits it to the window";
                painter.text(canvas.left_bottom() + Vec2::new(10.0, -10.0), Align2::LEFT_BOTTOM, hint, font.clone(), dim);
            }
        }
        (None, None, false) => {
            let what = match media.kind {
                Media::Audio => "no picture in a sound file",
                _ => "nothing to show",
            };
            painter.text(canvas.center(), Align2::CENTER_CENTER, what, font.clone(), dim);
        }
    }

    if slider_h > 0.0 {
        let seconds = media.info.as_ref().and_then(|i| i.seconds).unwrap_or(0.0);
        let (at, mut wanted) = (media.at, media.wanted);
        let strip = Rect::from_min_max(egui::Pos2::new(area.min.x, area.max.y - slider_h), area.max);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(strip.shrink2(Vec2::new(10.0, 6.0))));
        child.horizontal(|ui| {
            ui.label(egui::RichText::new(stamp(at)).font(font.clone()).color(text));
            // A slider keeps its own width unless told; the strip is
            // the right width for it.
            ui.spacing_mut().slider_width = (ui.available_width() - 60.0).max(80.0);
            let slider = egui::Slider::new(&mut wanted, 0.0..=seconds.max(0.1)).show_value(false);
            let response = ui.add(slider);
            ui.label(egui::RichText::new(stamp(seconds)).font(font.clone()).color(dim));
            // The frame is pulled when you let go, not while dragging.
            if response.drag_stopped() || (response.changed() && !response.dragged()) {
                if let Some(media) = app.docs[app.active].media.as_mut() {
                    media.wanted = wanted;
                    media.pending = true;
                }
            } else if response.dragged() {
                if let Some(media) = app.docs[app.active].media.as_mut() {
                    media.wanted = wanted;
                }
            }
        });
        let _ = accent;
    }
    true
}

/// `1:04` for the slider's ends.
fn stamp(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn open_externally(app: &mut QellApp, name: &str) {
    match preview::open_externally(std::path::Path::new(name)) {
        Ok(()) => app.notify(format!("Opened {name} outside Qell.")),
        Err(e) => app.notify_error(e),
    }
}

/// `f` fits, `1` shows it at its own size, `o` opens it elsewhere, and
/// the arrows step through a video.
fn keys(app: &mut QellApp, ctx: &egui::Context) {
    if app.focus != crate::app::Focus::Editor || app.modal_open() {
        return;
    }
    let name = app.docs.get(app.active).and_then(|d| d.buffer.path()).map(|p| p.display().to_string());
    let seconds = app
        .docs
        .get(app.active)
        .and_then(|d| d.media.as_ref())
        .and_then(|m| m.info.as_ref())
        .and_then(|i| i.seconds)
        .unwrap_or(0.0);
    let events = ctx.input(|i| i.events.clone());
    for event in events {
        let egui::Event::Key { key, pressed: true, modifiers, .. } = event else { continue };
        if modifiers.ctrl || modifiers.command || modifiers.alt {
            continue;
        }
        let Some(media) = app.docs.get_mut(app.active).and_then(|d| d.media.as_mut()) else { return };
        // A step of a second, or ten with shift, like a player.
        let step = if modifiers.shift { 10.0 } else { 1.0 };
        match key {
            Key::F => media.actual_size = false,
            Key::Num1 => media.actual_size = true,
            Key::ArrowRight | Key::L if media.kind == Media::Video => {
                media.wanted = (media.wanted + step).min(seconds);
                media.pending = true;
            }
            Key::ArrowLeft | Key::H if media.kind == Media::Video => {
                media.wanted = (media.wanted - step).max(0.0);
                media.pending = true;
            }
            Key::O => {
                if let Some(name) = &name {
                    let name = name.clone();
                    open_externally(app, &name);
                    return;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slider_ends_read_as_times() {
        assert_eq!(stamp(0.0), "0:00");
        assert_eq!(stamp(64.4), "1:04");
        assert_eq!(stamp(3725.0), "1:02:05");
    }
}
