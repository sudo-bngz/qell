//! Dead keys and input methods in Qell's own text areas (editor,
//! terminal). On macOS, `⌥N` then a space gives `~`, `⌥E` then `e` gives
//! `é`, and Chinese or Japanese input composes the same way. The system
//! only does that for a window that says it takes composed text, at a
//! place; egui's own text boxes say so, the editor and the terminal must
//! too. What gets composed arrives as an IME commit, taken here as typed
//! text.

use egui::{Context, Event, ImeEvent, Rect};

/// Composed text is welcome now, at `cursor` inside `area` (call each
/// frame the text area has the keyboard).
pub fn allow(ctx: &Context, area: Rect, cursor: Rect) {
    ctx.output_mut(|o| o.ime = Some(egui::output::IMEOutput { rect: area, cursor_rect: cursor }));
}

/// The frame's events with composed text (an IME commit) as typed text;
/// the rest of the composition (start, preview, end) is ignored by the
/// views as before.
pub fn as_text(events: Vec<Event>) -> Vec<Event> {
    events
        .into_iter()
        .map(|e| match e {
            Event::Ime(ImeEvent::Commit(t)) if !t.is_empty() => Event::Text(t),
            e => e,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commits_become_text() {
        let out = as_text(vec![
            Event::Ime(ImeEvent::Enabled),
            Event::Ime(ImeEvent::Preedit("˜".into())),
            Event::Ime(ImeEvent::Commit("~".into())),
            Event::Text("a".into()),
        ]);
        let texts: Vec<&str> = out.iter().filter_map(|e| if let Event::Text(t) = e { Some(t.as_str()) } else { None }).collect();
        assert_eq!(texts, ["~", "a"]);
        // Same count: indexes still point at the frame's events.
        assert_eq!(out.len(), 4);
    }
}
