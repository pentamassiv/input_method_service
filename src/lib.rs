/*! Manages zwp_input_method_v2 protocol.
 *
 * Library module.
 */

use std::cell::RefCell;
use std::num::Wrapping;
use std::rc::Rc;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{event_enum, Filter, Main};
use wayland_protocols::unstable::text_input::v3::client::zwp_text_input_v3::{
    ChangeCause, ContentHint, ContentPurpose,
};
use zwp_input_method::input_method_unstable_v2::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use zwp_input_method::input_method_unstable_v2::zwp_input_method_v2::Event as InputMethodEvent;
use zwp_input_method::input_method_unstable_v2::zwp_input_method_v2::ZwpInputMethodV2;

pub enum SubmitError {
    /// The input method had not been activated
    NotActive,
}

event_enum!(
    Events |
    InputMethod => ZwpInputMethodV2
);

pub trait KeyboardVisability {
    fn show_keyboard(&self);
    fn hide_keyboard(&self);
}

pub trait HintPurpose {
    fn set_hint_purpose(&self, content_hint: ContentHint, content_purpose: ContentPurpose);
}

/// Describes the desired state of the input method as requested by the server
#[derive(Clone)]
struct IMProtocolState {
    surrounding_text: String,
    surrounding_cursor: u32,
    content_purpose: ContentPurpose,
    content_hint: ContentHint,
    text_change_cause: ChangeCause,
    active: bool,
}

impl Default for IMProtocolState {
    fn default() -> IMProtocolState {
        IMProtocolState {
            surrounding_text: String::new(),
            surrounding_cursor: 0, // TODO: mark that there's no cursor
            content_hint: ContentHint::None,
            content_purpose: ContentPurpose::Normal,
            text_change_cause: ChangeCause::InputMethod,
            active: false,
        }
    }
}

pub struct IMService<T: 'static + KeyboardVisability + HintPurpose> {
    pub im: Main<ZwpInputMethodV2>,
    connector: &'static T,
    pending: IMProtocolState,
    current: IMProtocolState, // turn current into an idiomatic representation?
    preedit_string: String,
    serial: Wrapping<u32>,
}

impl<T: 'static + KeyboardVisability + HintPurpose> IMService<T> {
    pub fn new(
        seat: &WlSeat,
        im_manager: ZwpInputMethodManagerV2,
        connector: &'static T,
    ) -> Rc<RefCell<IMService<T>>> {
        let im = im_manager.get_input_method(seat);
        let im_service = IMService {
            im,
            connector,
            pending: IMProtocolState::default(),
            current: IMProtocolState::default(),
            preedit_string: String::new(),
            serial: Wrapping(0u32),
        };
        let im_service = Rc::new(RefCell::new(im_service));
        let im_service_ref = Rc::clone(&im_service);
        im_service.borrow_mut().assign_filter(im_service_ref);
        im_service
    }

    fn assign_filter(&self, im_service: Rc<RefCell<IMService<T>>>) {
        let im_service_ref = im_service;
        let filter = Filter::new(move |event, _, _| match event {
            Events::InputMethod { event, .. } => match event {
                InputMethodEvent::Activate => im_service_ref.borrow_mut().handle_activate(),
                InputMethodEvent::Deactivate => im_service_ref.borrow_mut().handle_deactivate(),
                InputMethodEvent::SurroundingText {
                    text,
                    cursor,
                    anchor,
                } => im_service_ref
                    .borrow_mut()
                    .handle_surrounding_text(text, cursor, anchor),
                InputMethodEvent::TextChangeCause { cause } => {
                    im_service_ref.borrow_mut().handle_text_change_cause(cause)
                }
                InputMethodEvent::ContentType { hint, purpose } => im_service_ref
                    .borrow_mut()
                    .handle_content_type(hint, purpose),
                InputMethodEvent::Done => im_service_ref.borrow_mut().handle_done(),
                InputMethodEvent::Unavailable => im_service_ref.borrow_mut().handle_unavailable(),
                _ => (),
            },
        });
        self.im.assign(filter);
    }

    pub fn commit_string(&self, text: String) -> Result<(), SubmitError> {
        match self.current.active {
            true => {
                self.im.commit_string(text);
                Ok(())
            }
            false => Err(SubmitError::NotActive),
        }
    }

    pub fn delete_surrounding_text(&self, before: u32, after: u32) -> Result<(), SubmitError> {
        match self.current.active {
            true => {
                self.im.delete_surrounding_text(before, after);
                Ok(())
            }
            false => Err(SubmitError::NotActive),
        }
    }

    pub fn commit(&mut self) -> Result<(), SubmitError> {
        match self.current.active {
            true => {
                self.im.commit(self.serial.0);
                self.serial += Wrapping(1u32);
                Ok(())
            }
            false => Err(SubmitError::NotActive),
        }
    }

    pub fn is_active(&self) -> bool {
        self.current.active
    }

    fn handle_activate(&mut self) {
        self.preedit_string = String::new();
        self.pending = IMProtocolState {
            active: true,
            ..IMProtocolState::default()
        };
    }

    fn handle_deactivate(&mut self) {
        self.pending = IMProtocolState {
            active: false,
            ..self.pending.clone()
        };
    }
    fn handle_surrounding_text(&mut self, text: String, cursor: u32, anchor: u32) {
        self.pending = IMProtocolState {
            surrounding_text: text,
            surrounding_cursor: cursor,
            ..self.pending.clone()
        };
    }

    fn handle_text_change_cause(&mut self, cause: ChangeCause) {
        self.pending = IMProtocolState {
            text_change_cause: cause,
            ..self.pending.clone()
        };
    }

    fn handle_content_type(&mut self, hint: ContentHint, purpose: ContentPurpose) {
        self.pending = IMProtocolState {
            content_hint: hint,
            content_purpose: purpose,
            ..self.pending.clone()
        };
    }

    fn handle_done(&mut self) {
        let active_changed = self.current.active ^ self.pending.active;

        self.current = self.pending.clone();
        self.pending = IMProtocolState {
            active: self.current.active,
            ..IMProtocolState::default()
        };

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

    fn handle_unavailable(&mut self) {
        self.im.destroy();
        self.current.active = false;
        self.connector.hide_keyboard();
    }
}
