//! This crate provides an easy to use interface to use the zwp_input_method_v2 protocol
//!
use std::convert::TryInto;
use std::num::Wrapping;
use std::sync::{Arc, Mutex};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Filter, Main};
use wayland_protocols::unstable::text_input::v3::client::zwp_text_input_v3::{
    ChangeCause, ContentHint, ContentPurpose,
};
#[macro_use]
extern crate log;
use zwp_input_method::input_method_unstable_v2::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use zwp_input_method::input_method_unstable_v2::zwp_input_method_v2::Event as InputMethodEvent;
use zwp_input_method::input_method_unstable_v2::zwp_input_method_v2::ZwpInputMethodV2;

#[derive(Debug, Clone)]
pub enum SubmitError {
    /// Input method was not activ
    NotActive,
}

mod event_enum {
    use wayland_client::event_enum;
    use zwp_input_method::input_method_unstable_v2::zwp_input_method_v2::ZwpInputMethodV2;
    event_enum!(
        Events |
        InputMethod => ZwpInputMethodV2
    );
}

pub trait KeyboardVisibility {
    fn show_keyboard(&self);
    fn hide_keyboard(&self);
}

pub trait HintPurpose {
    fn set_hint_purpose(&self, content_hint: ContentHint, content_purpose: ContentPurpose);
}

/// Describes the desired state of the input method as requested by the server
#[derive(Clone, Debug)]
struct IMProtocolState {
    surrounding_text: String,
    cursor: u32,
    content_purpose: ContentPurpose,
    content_hint: ContentHint,
    text_change_cause: ChangeCause,
    active: bool,
}

impl Default for IMProtocolState {
    fn default() -> IMProtocolState {
        IMProtocolState {
            surrounding_text: String::new(),
            cursor: 0,
            content_hint: ContentHint::None,
            content_purpose: ContentPurpose::Normal,
            text_change_cause: ChangeCause::InputMethod,
            active: false,
        }
    }
}

#[derive(Clone, Debug)]
struct IMServiceArc<T: 'static + KeyboardVisibility + HintPurpose> {
    im: Main<ZwpInputMethodV2>,
    connector: T,
    pending: IMProtocolState,
    current: IMProtocolState,
    serial: Wrapping<u32>,
}

impl<T: 'static + KeyboardVisibility + HintPurpose> IMServiceArc<T> {
    fn new(
        seat: &WlSeat,
        im_manager: Main<ZwpInputMethodManagerV2>,
        connector: T,
    ) -> Arc<Mutex<IMServiceArc<T>>> {
        let im = im_manager.get_input_method(seat);
        let im_service = IMServiceArc {
            im,
            connector,
            pending: IMProtocolState::default(),
            current: IMProtocolState::default(),
            serial: Wrapping(0u32),
        };
        let im_service = Arc::new(Mutex::new(im_service));
        let im_service_ref = Arc::clone(&im_service);
        im_service.lock().unwrap().assign_filter(im_service_ref);
        info!("New IMService was created");
        im_service
    }

    fn assign_filter(&self, im_service: Arc<Mutex<IMServiceArc<T>>>) {
        let filter = Filter::new(move |event, _, _| match event {
            event_enum::Events::InputMethod { event, .. } => match event {
                InputMethodEvent::Activate => im_service.lock().unwrap().handle_activate(),
                InputMethodEvent::Deactivate => im_service.lock().unwrap().handle_deactivate(),
                InputMethodEvent::SurroundingText {
                    text,
                    cursor,
                    anchor,
                } => im_service
                    .lock()
                    .unwrap()
                    .handle_surrounding_text(text, cursor, anchor),
                InputMethodEvent::TextChangeCause { cause } => {
                    im_service.lock().unwrap().handle_text_change_cause(cause)
                }
                InputMethodEvent::ContentType { hint, purpose } => im_service
                    .lock()
                    .unwrap()
                    .handle_content_type(hint, purpose),
                InputMethodEvent::Done => im_service.lock().unwrap().handle_done(),
                InputMethodEvent::Unavailable => im_service.lock().unwrap().handle_unavailable(),
                _ => (),
            },
        });
        self.im.assign(filter);
        info!("The filter was assigned to Main<ZwpInputMethodV2>");
    }

    fn commit_string(&mut self, text: String) -> Result<(), SubmitError> {
        info!("Commit string '{}'", text);
        match self.current.active {
            true => {
                let cursor_position = self.pending.cursor.try_into().unwrap(); // Converts u32 of cursor to usize
                self.pending
                    .surrounding_text
                    .insert_str(cursor_position, &text);
                self.pending.cursor += text.len() as u32;
                self.im.commit_string(text);
                Ok(())
            }
            false => Err(SubmitError::NotActive),
        }
    }

    fn delete_surrounding_text(&mut self, before: u32, after: u32) -> Result<(), SubmitError> {
        info!(
            "Delete the surrounding text ({} bytes before and {} after the cursor at {})",
            before, after, self.pending.active
        );
        match self.current.active {
            true => {
                let cursor_position: usize = self.pending.cursor.try_into().unwrap(); // Converts u32 of cursor to usize
                self.pending.surrounding_text.replace_range(
                    cursor_position - (before as usize)..cursor_position + (after as usize),
                    "",
                );
                self.im.delete_surrounding_text(before, after);
                Ok(())
            }
            false => Err(SubmitError::NotActive),
        }
    }

    fn commit(&mut self) -> Result<(), SubmitError> {
        info!("Commit the changes");
        match self.current.active {
            true => {
                self.im.commit(self.serial.0);
                self.serial += Wrapping(1u32);
                self.pending_becomes_current();
                Ok(())
            }
            false => Err(SubmitError::NotActive),
        }
    }

    fn is_active(&self) -> bool {
        self.current.active
    }

    fn get_surrounding_text(&self) -> String {
        info!("Requested surrounding text");
        self.current.surrounding_text.clone()
    }

    fn handle_activate(&mut self) {
        info!("handle_activate() was called");
        self.pending = IMProtocolState {
            active: true,
            ..IMProtocolState::default()
        };
    }

    fn handle_deactivate(&mut self) {
        info!("handle_deactivate() was called");
        self.pending.active = false;
    }

    fn handle_surrounding_text(&mut self, text: String, cursor: u32, anchor: u32) {
        info!("handle_surrounding_text() was called");
        self.pending.surrounding_text = text;
        self.pending.cursor = cursor;
    }

    fn handle_text_change_cause(&mut self, cause: ChangeCause) {
        info!("handle_text_change_cause() was called");
        self.pending.text_change_cause = cause;
    }

    fn handle_content_type(&mut self, hint: ContentHint, purpose: ContentPurpose) {
        info!("handle_content_type() was called");
        self.pending.content_hint = hint;
        self.pending.content_purpose = purpose;
    }

    fn handle_done(&mut self) {
        info!("handle_done() was called");
        self.pending_becomes_current();
    }

    fn handle_unavailable(&mut self) {
        info!("handle_unavailable() was called");
        self.im.destroy();
        self.current.active = false;
        self.connector.hide_keyboard();
    }

    fn pending_becomes_current(&mut self) {
        info!("The pending protocol state became the current state");
        let active_changed = self.current.active ^ self.pending.active;

        self.current = self.pending.clone();
        //self.pending = IMProtocolState {
        //    active: self.current.active,
        //    ..IMProtocolState::default()
        //};

        if active_changed {
            if self.current.active {
                self.connector.show_keyboard();
                self.connector
                    .set_hint_purpose(self.current.content_hint, self.current.content_purpose);
            } else {
                self.connector.hide_keyboard();
            };
        }
    }
}

#[derive(Clone, Debug)]
pub struct IMService<T: 'static + KeyboardVisibility + HintPurpose> {
    im_service_arc: Arc<Mutex<IMServiceArc<T>>>,
}

impl<T: 'static + KeyboardVisibility + HintPurpose> IMService<T> {
    pub fn new(
        seat: &WlSeat,
        im_manager: Main<ZwpInputMethodManagerV2>,
        connector: T,
    ) -> IMService<T> {
        let im_service_arc = IMServiceArc::new(seat, im_manager, connector);
        IMService { im_service_arc }
    }

    pub fn commit_string(&self, text: String) -> Result<(), SubmitError> {
        self.im_service_arc.lock().unwrap().commit_string(text)
    }

    pub fn delete_surrounding_text(&self, before: u32, after: u32) -> Result<(), SubmitError> {
        info!("Delete surrounding text ");
        self.im_service_arc
            .lock()
            .unwrap()
            .delete_surrounding_text(before, after)
    }

    pub fn commit(&self) -> Result<(), SubmitError> {
        self.im_service_arc.lock().unwrap().commit()
    }

    pub fn is_active(&self) -> bool {
        self.im_service_arc.lock().unwrap().is_active()
    }

    pub fn get_surrounding_text(&self) -> String {
        self.im_service_arc.lock().unwrap().get_surrounding_text()
    }
}
