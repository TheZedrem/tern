//! Dragging files that are already on this disk out of the browser (spec
//! 0013, Drag out and flow 11): a Local row, or an SFTP edit's copy. Out
//! of the window, GPUI hands the OS the path as a file drag; inside it,
//! nothing takes the drag. GPUI 0.3.11 starts the OS drag on Wayland only
//! (T-092 is Windows).

use crate::terminal::clean_title;
use gpui::{
    AppContext, Context, ExternalDragPayload, FileDragPaths, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_component::ActiveTheme;
use std::path::PathBuf;

/// What a drag out carries: one path on this disk, and whether it's a
/// folder.
pub(super) struct DraggedFile {
    path: PathBuf,
    folder: bool,
}

/// The chip that follows the pointer inside the window.
struct DragPreview(SharedString);

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px(px(8.))
            .py(px(2.))
            .rounded(px(4.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .text_xs()
            .child(self.0.clone())
    }
}

/// Makes `element` drag `path` out of the window, named `name` while it's
/// inside.
pub(super) fn drag_out<E: StatefulInteractiveElement>(
    element: E,
    path: PathBuf,
    folder: bool,
    name: &str,
) -> E {
    let name: SharedString = clean_title(name).into();
    element
        .on_drag(DraggedFile { path, folder }, move |_, _, _, cx| {
            cx.new(|_| DragPreview(name.clone()))
        })
        .external_drag_payload(|dragged: &DraggedFile, _, _| {
            Some(ExternalDragPayload::Files(FileDragPaths::new([(
                dragged.path.clone(),
                dragged.folder,
            )])))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        InteractiveElement, Modifiers, MouseButton, TestAppContext, VisualTestContext, point,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    /// What a drop target saw: each drag's path and folder flag.
    type Seen = Rc<RefCell<Vec<(PathBuf, bool)>>>;

    /// A row made draggable by [`drag_out`] at x 0..100, and a stand-in
    /// for wherever the drag lands at x 100..200.
    struct Rows {
        path: PathBuf,
        folder: bool,
        seen: Seen,
    }

    impl Render for Rows {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let seen = self.seen.clone();
            div()
                .size_full()
                .flex()
                .child(drag_out(
                    div().id("row").w(px(100.)).h(px(100.)),
                    self.path.clone(),
                    self.folder,
                    "notes.txt",
                ))
                .child(div().id("landing").w(px(100.)).h(px(100.)).on_drop(
                    move |dragged: &DraggedFile, _, _| {
                        seen.borrow_mut()
                            .push((dragged.path.clone(), dragged.folder));
                    },
                ))
        }
    }

    /// Presses on the row, drags it to the landing spot and lets go.
    fn drag(cx: &mut VisualTestContext) {
        let none = Modifiers::default();
        cx.simulate_mouse_down(point(px(10.), px(10.)), MouseButton::Left, none);
        cx.simulate_mouse_move(point(px(40.), px(10.)), MouseButton::Left, none);
        cx.simulate_mouse_move(point(px(150.), px(10.)), MouseButton::Left, none);
        cx.simulate_mouse_up(point(px(150.), px(10.)), MouseButton::Left, none);
    }

    /// Spec 0013 flow 11 and AC17: a Local row's drag carries that row's
    /// path, and whether it's a folder, which is what the OS is handed
    /// once the drag leaves the window. GPUI keeps its test window's
    /// record of the OS drag crate-private, so this checks the value the
    /// payload is built from; that a remote row never drags is the row
    /// renderer's and needs a connected browser, so it isn't covered here.
    #[gpui::test]
    fn a_dragged_row_carries_its_path(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        for (path, folder) in [
            (PathBuf::from("/home/a/notes.txt"), false),
            (PathBuf::from("/home/a/docs"), true),
        ] {
            let seen = Seen::default();
            let (_, window) = cx.add_window_view({
                let (path, seen) = (path.clone(), seen.clone());
                move |_, _| Rows { path, folder, seen }
            });
            drag(window);
            assert_eq!(*seen.borrow(), [(path, folder)]);
        }
    }
}
