//! The pointer and the keyboard, on the shell's surfaces.
//!
//! A button on a desktop is a click on the wallpaper: the right one opens
//! the root menu where it was pressed. The pointer and the buttons on the
//! menus' surface, and every key while it has the keyboard, are the menus'.
//! The pointer is drawn as the cursor theme's arrow on every surface of the
//! shell's, rather than as whatever the last window left it as.

use smithay_client_toolkit::{
    delegate_keyboard, delegate_pointer, delegate_seat,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{
            CursorIcon, PointerEvent, PointerEventKind, PointerHandler, ThemeSpec, ThemedPointer,
        },
    },
};
use wayland_client::{
    Connection, QueueHandle,
    globals::GlobalList,
    protocol::{wl_keyboard, wl_pointer, wl_seat, wl_surface},
};

use super::App;
use crate::update::{Button, Event, Key};

/// Linux's button codes, from `linux/input-event-codes.h`, which Wayland
/// carries as they are.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

/// The seat, and the pointer and keyboard on it.
pub(super) struct Seat {
    state: SeatState,
    pointer: Option<ThemedPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
}

impl Seat {
    pub(super) fn new(globals: &GlobalList, qh: &QueueHandle<App>) -> Self {
        Self {
            state: SeatState::new(globals, qh),
            pointer: None,
            keyboard: None,
        }
    }
}

impl App {
    /// A key pressed, or repeating, on the menus.
    fn key(&mut self, event: &KeyEvent) {
        if let Some(key) = key(event) {
            self.menu_event(Event::Key(key));
        }
    }
}

/// What a key is to a menu, if anything.
fn key(event: &KeyEvent) -> Option<Key> {
    Some(match event.keysym {
        Keysym::Up | Keysym::KP_Up => Key::Up,
        Keysym::Down | Keysym::KP_Down => Key::Down,
        Keysym::Left | Keysym::KP_Left => Key::Left,
        Keysym::Right | Keysym::KP_Right => Key::Right,
        Keysym::Home | Keysym::KP_Home | Keysym::Page_Up => Key::Home,
        Keysym::End | Keysym::KP_End | Keysym::Page_Down => Key::End,
        Keysym::Return | Keysym::KP_Enter => Key::Enter,
        Keysym::Escape => Key::Escape,
        Keysym::BackSpace => Key::Backspace,
        _ => Key::Text(
            event
                .utf8
                .clone()
                .filter(|text| text.chars().all(|c| !c.is_control()) && !text.is_empty())?,
        ),
    })
}

fn button(code: u32) -> Button {
    match code {
        BTN_LEFT => Button::Left,
        BTN_RIGHT => Button::Right,
        BTN_MIDDLE => Button::Middle,
        _ => Button::Other,
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat.state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Pointer if self.seat.pointer.is_none() => {
                let cursor = self.canvas.compositor.create_surface(qh);
                match self.seat.state.get_pointer_with_theme(
                    qh,
                    &seat,
                    self.canvas.shm().wl_shm(),
                    cursor,
                    ThemeSpec::default(),
                ) {
                    Ok(pointer) => self.seat.pointer = Some(pointer),
                    Err(error) => tracing::warn!("no pointer: {error}"),
                }
            }
            Capability::Keyboard if self.seat.keyboard.is_none() => {
                match self.seat.state.get_keyboard_with_repeat(
                    qh,
                    &seat,
                    None,
                    self.handle.clone(),
                    Box::new(|app, _, event| app.key(&event)),
                ) {
                    Ok(keyboard) => self.seat.keyboard = Some(keyboard),
                    Err(error) => tracing::warn!("no keyboard: {error}"),
                }
            }
            _ => {}
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Pointer => {
                if let Some(pointer) = self.seat.pointer.take() {
                    pointer.pointer().release();
                }
            }
            Capability::Keyboard => {
                if let Some(keyboard) = self.seat.keyboard.take() {
                    keyboard.release();
                }
            }
            _ => {}
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        connection: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let on_menus = self.menus.owns(&event.surface);
            let at = event.position;
            match event.kind {
                PointerEventKind::Enter { .. } => {
                    if let Some(pointer) = &self.seat.pointer
                        && let Err(error) = pointer.set_cursor(connection, CursorIcon::Default)
                    {
                        tracing::debug!("no cursor drawn: {error}");
                    }
                    if on_menus {
                        self.menu_event(Event::Motion(at));
                    }
                }
                PointerEventKind::Motion { .. } if on_menus => self.menu_event(Event::Motion(at)),
                PointerEventKind::Press { .. } if on_menus => self.menu_event(Event::Press(at)),
                PointerEventKind::Release { .. } if on_menus => self.menu_event(Event::Release(at)),
                PointerEventKind::Press { button: code, .. } => {
                    self.desktop_press(&event.surface, at, button(code))
                }
                _ => {}
            }
        }
    }
}

impl App {
    /// A button went down on a surface of the shell's other than the menus'.
    #[cfg(feature = "wallpaper")]
    fn desktop_press(&mut self, surface: &wl_surface::WlSurface, at: (f64, f64), button: Button) {
        let Some((output, (width, height))) = self.desktops.at(surface) else {
            return;
        };
        let event = Event::DesktopPress {
            output: output.to_owned(),
            area: crate::layout::Rect::new(0, 0, width as i32, height as i32),
            at,
            button,
        };
        self.menu_event(event);
    }

    #[cfg(not(feature = "wallpaper"))]
    fn desktop_press(&mut self, _: &wl_surface::WlSurface, _: (f64, f64), _: Button) {}
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        self.menus.keyboard(surface, true);
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        // Something else took the keyboard: a window an agent focused, a
        // screen locker. A menu without it closes, as a menu does.
        if self.menus.keyboard(surface, false) {
            self.menu_event(Event::KeyboardLost);
        }
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key(&event);
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key(&event);
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
    }
}

delegate_seat!(App);
delegate_pointer!(App);
delegate_keyboard!(App);
