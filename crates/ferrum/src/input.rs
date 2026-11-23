/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Traducción de eventos de winit (ventana del sistema) a eventos de Servo.

use keyboard_types::{Code, Key, KeyState, Location, Modifiers, NamedKey};
use servo::{Cursor, MouseButton};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{self, KeyLocation, ModifiersState, PhysicalKey};
use winit::window::CursorIcon;

pub fn modifiers(state: ModifiersState) -> Modifiers {
    let mut modifiers = Modifiers::empty();
    modifiers.set(Modifiers::SHIFT, state.shift_key());
    modifiers.set(Modifiers::CONTROL, state.control_key());
    modifiers.set(Modifiers::ALT, state.alt_key());
    modifiers.set(Modifiers::META, state.super_key());
    modifiers
}

/// Convierte una pulsación de winit en el evento W3C que espera Servo.
pub fn keyboard_event(event: &KeyEvent, state: ModifiersState) -> servo::KeyboardEvent {
    servo::KeyboardEvent::new(keyboard_types::KeyboardEvent {
        state: match event.state {
            ElementState::Pressed => KeyState::Down,
            ElementState::Released => KeyState::Up,
        },
        key: key(event),
        code: code(event.physical_key),
        location: match event.location {
            KeyLocation::Standard => Location::Standard,
            KeyLocation::Left => Location::Left,
            KeyLocation::Right => Location::Right,
            KeyLocation::Numpad => Location::Numpad,
        },
        modifiers: modifiers(state),
        repeat: event.repeat,
        is_composing: false,
    })
}

fn key(event: &KeyEvent) -> Key {
    match &event.logical_key {
        keyboard::Key::Character(character) => {
            // `text` ya trae aplicadas las teclas muertas (´ + a = á).
            let text = event
                .text
                .as_ref()
                .filter(|text| !text.chars().any(char::is_control))
                .map(|text| text.to_string());
            Key::Character(text.unwrap_or_else(|| character.to_string()))
        },
        keyboard::Key::Named(keyboard::NamedKey::Space) => Key::Character(" ".into()),
        keyboard::Key::Named(named) => {
            let name = match named {
                keyboard::NamedKey::Super => "Meta".to_owned(),
                other => format!("{other:?}"),
            };
            Key::Named(name.parse().unwrap_or(NamedKey::Unidentified))
        },
        keyboard::Key::Dead(_) => Key::Named(NamedKey::Dead),
        keyboard::Key::Unidentified(_) => Key::Named(NamedKey::Unidentified),
    }
}

fn code(physical: PhysicalKey) -> Code {
    let PhysicalKey::Code(code) = physical else {
        return Code::Unidentified;
    };
    let name = match code {
        keyboard::KeyCode::SuperLeft => "MetaLeft".to_owned(),
        keyboard::KeyCode::SuperRight => "MetaRight".to_owned(),
        other => format!("{other:?}"),
    };
    name.parse().unwrap_or(Code::Unidentified)
}

pub fn mouse_button(button: winit::event::MouseButton) -> MouseButton {
    match button {
        winit::event::MouseButton::Left => MouseButton::Primary,
        winit::event::MouseButton::Right => MouseButton::Secondary,
        winit::event::MouseButton::Middle => MouseButton::Auxiliary,
        winit::event::MouseButton::Back => MouseButton::Back,
        winit::event::MouseButton::Forward => MouseButton::Forward,
        winit::event::MouseButton::Other(value) => MouseButton::Other(value),
    }
}

/// Cursor de Servo → cursor del sistema. `None` = ocultar el cursor.
pub fn cursor_icon(cursor: Cursor) -> Option<CursorIcon> {
    Some(match cursor {
        Cursor::None => return None,
        Cursor::Default => CursorIcon::Default,
        Cursor::Pointer => CursorIcon::Pointer,
        Cursor::ContextMenu => CursorIcon::ContextMenu,
        Cursor::Help => CursorIcon::Help,
        Cursor::Progress => CursorIcon::Progress,
        Cursor::Wait => CursorIcon::Wait,
        Cursor::Cell => CursorIcon::Cell,
        Cursor::Crosshair => CursorIcon::Crosshair,
        Cursor::Text => CursorIcon::Text,
        Cursor::VerticalText => CursorIcon::VerticalText,
        Cursor::Alias => CursorIcon::Alias,
        Cursor::Copy => CursorIcon::Copy,
        Cursor::Move => CursorIcon::Move,
        Cursor::NoDrop => CursorIcon::NoDrop,
        Cursor::NotAllowed => CursorIcon::NotAllowed,
        Cursor::Grab => CursorIcon::Grab,
        Cursor::Grabbing => CursorIcon::Grabbing,
        Cursor::EResize => CursorIcon::EResize,
        Cursor::NResize => CursorIcon::NResize,
        Cursor::NeResize => CursorIcon::NeResize,
        Cursor::NwResize => CursorIcon::NwResize,
        Cursor::SResize => CursorIcon::SResize,
        Cursor::SeResize => CursorIcon::SeResize,
        Cursor::SwResize => CursorIcon::SwResize,
        Cursor::WResize => CursorIcon::WResize,
        Cursor::EwResize => CursorIcon::EwResize,
        Cursor::NsResize => CursorIcon::NsResize,
        Cursor::NeswResize => CursorIcon::NeswResize,
        Cursor::NwseResize => CursorIcon::NwseResize,
        Cursor::ColResize => CursorIcon::ColResize,
        Cursor::RowResize => CursorIcon::RowResize,
        Cursor::AllScroll => CursorIcon::AllScroll,
        Cursor::ZoomIn => CursorIcon::ZoomIn,
        Cursor::ZoomOut => CursorIcon::ZoomOut,
    })
}
