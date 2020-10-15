//! This crate provides an easy to use interface for the zwp_input_method_v2 protocol
//! It allows a wayland client to serve as an input method for other wayland-clients. This could be used for virtual keyboards
//!
#[macro_use]
extern crate log;

use std::convert::TryInto;
use std::num::Wrapping;
use std::sync::{Arc, Mutex};
use wayland_client::{protocol::wl_seat::WlSeat, Filter, Main};
use wayland_protocols::unstable::text_input::v3::client::zwp_text_input_v3::{
    ChangeCause, ContentHint, ContentPurpose,
};
use zwp_input_method::input_method_unstable_v2::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use zwp_input_method::input_method_unstable_v2::zwp_input_method_v2::{
    Event as InputMethodEvent, ZwpInputMethodV2,
};

#[derive(Debug, Clone)]
/// Error when sending a request to the wayland-client
pub enum SubmitError {
    /// Input method was not activ
    NotActive,
}

// Mandatory conversion to apply filter to ZwpInputMethodV2
mod event_enum {
    use wayland_client::event_enum;
    use zwp_input_method::input_method_unstable_v2::zwp_input_method_v2::ZwpInputMethodV2;
    event_enum!(
        Events |
        InputMethod => ZwpInputMethodV2
    );
}

/// Trait to get notified when the keyboard should be shown or hidden
/// If the user clicks for example on a text field, the method show_keyboard() is called
pub trait KeyboardVisibility {
    fn show_keyboard(&self);
    fn hide_keyboard(&self);
}

/// Trait to get notified when the hint or the purpose of the content changes
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
/// Manages the pending state and the current state of the input method.
/// It is called IMServiceArc and not IMService because the new() method
/// wraps IMServiceArc and returns Arc<Mutex<IMServiceArc<T>>>. This is required because it's state could get changed by multiple threads.
/// One thread could handle requests while the other one handles events from the wayland-server
struct IMServiceArc<T: 'static + KeyboardVisibility + HintPurpose> {
    im: Main<ZwpInputMethodV2>,
    connector: T,
    pending: IMProtocolState,
    current: IMProtocolState,
    serial: Wrapping<u32>,
}

impl<T: 'static + KeyboardVisibility + HintPurpose> IMServiceArc<T> {
    /// Creates a new IMServiceArc wrapped in Arc<Mutex< >>
    fn new(
        seat: &WlSeat,
        im_manager: Main<ZwpInputMethodManagerV2>,
        connector: T,
    ) -> Arc<Mutex<IMServiceArc<T>>> {
        // Get ZwpInputMethodV2 from ZwpInputMethodManagerV2
        let im = im_manager.get_input_method(seat);

        // Create IMServiceArc with default values
        let im_service = IMServiceArc {
            im,
            connector,
            pending: IMProtocolState::default(),
            current: IMProtocolState::default(),
            serial: Wrapping(0u32),
        };

        // Wrap IMServiceArc to allow mutability from multiple threads
        let im_service = Arc::new(Mutex::new(im_service));

        // Clone the reference to move it to the filter
        let im_service_ref = Arc::clone(&im_service);
        // Assigns a filter to the wayland event queue to handle events for ZwpInputMethodV2
        im_service.lock().unwrap().assign_filter(im_service_ref);
        info!("New IMService was created");
        // Return the wrapped IMServiceArc
        im_service
    }

    /// Assigns a filter to the wayland event queue to allow IMServiceArc to handle events from ZwpInputMethodV2
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

    /// Sends a 'commit_string' request to the wayland-server
    /// INPUTS: text -> Text that will be committed
    fn commit_string(&mut self, text: String) -> Result<(), SubmitError> {
        info!("Commit string '{}'", text);
        // Check if proxy is still alive. If the proxy was dead, the requests would fail silently
        match self.current.active {
            true => {
                let cursor_position = self.pending.cursor.try_into().unwrap(); // Converts u32 of cursor to usize
                                                                               // Append 'text' to the pending surrounding_text
                self.pending
                    .surrounding_text
                    .insert_str(cursor_position, &text);
                // Update the cursor
                self.pending.cursor += text.len() as u32;
                // Send the request to the wayland-server
                self.im.commit_string(text);
                Ok(())
            }
            false => Err(SubmitError::NotActive),
        }
    }

    /// Sends a 'delete_surrounding_text' request to the wayland server
    /// INPUTS:
    /// before -> number of chars to delete from the surrounding_text going left from the cursor
    /// after  -> number of chars to delete from the surrounding_text going right from the cursor
    fn delete_surrounding_text(&mut self, before: u32, after: u32) -> Result<(), SubmitError> {
        info!(
            "Send a request to the wayland server to delete {} chars before and {} after the cursor at {} from the surrounding text",
            before, after, self.pending.cursor
        );
        // Check if proxy is still alive. If the proxy was dead, the requests would fail silently
        match self.current.active {
            true => {
                // Limit 'before' and 'after' if they exceed the maximum
                let (before, after) = self.limit_before_after(before, after);
                // Update self.pending.surrounging_text and self.pending.cursor
                self.update_cursor_and_surrounding_text(before, after);
                // Send the delete_surrounding_text request to the wayland-server
                self.im.delete_surrounding_text(before as u32, after as u32);
                Ok(())
            }
            false => Err(SubmitError::NotActive),
        }
    }

    /// Sends a 'commit' request to the wayland server
    /// This makes the pending changes permanent
    fn commit(&mut self) -> Result<(), SubmitError> {
        info!("Commit the changes");
        // Check if proxy is still alive. If the proxy was dead, the requests would fail silently
        match self.current.active {
            true => {
                // Send request to wayland-server
                self.im.commit(self.serial.0);
                // Increase the serial
                self.serial += Wrapping(1u32);
                // Make pending changes permanent
                self.pending_becomes_current();
                Ok(())
            }
            false => Err(SubmitError::NotActive),
        }
    }

    /// Returns if the input method is currently active
    fn is_active(&self) -> bool {
        self.current.active
    }

    /// Returns the current surrounding_text
    fn get_surrounding_text(&self) -> String {
        info!("Requested surrounding text");
        self.current.surrounding_text.clone()
    }

    /// Handles the 'activate' event sent from the wayland server
    /// This method should never be called from the client
    fn handle_activate(&mut self) {
        info!("handle_activate() was called");
        self.pending = IMProtocolState {
            active: true,
            ..IMProtocolState::default()
        };
    }

    /// Handles the 'deactivate' event sent from the wayland server
    /// This method should never be called from the client
    fn handle_deactivate(&mut self) {
        info!("handle_deactivate() was called");
        self.pending.active = false;
    }

    /// Handles the 'surrounding_text' event sent from the wayland server
    /// This method should never be called from the client
    fn handle_surrounding_text(&mut self, text: String, cursor: u32, anchor: u32) {
        info!("handle_surrounding_text() was called");
        self.pending.surrounding_text = text;
        self.pending.cursor = cursor;
    }

    /// Handles the 'text_change_cause' event sent from the wayland server
    /// This method should never be called from the client
    fn handle_text_change_cause(&mut self, cause: ChangeCause) {
        info!("handle_text_change_cause() was called");
        self.pending.text_change_cause = cause;
    }

    /// Handles the 'content_type' event sent from the wayland server
    /// This method should never be called from the client
    fn handle_content_type(&mut self, hint: ContentHint, purpose: ContentPurpose) {
        info!("handle_content_type() was called");
        self.pending.content_hint = hint;
        self.pending.content_purpose = purpose;
    }

    /// Handles the 'done' event sent from the wayland server
    /// This method should never be called from the client
    fn handle_done(&mut self) {
        info!("handle_done() was called");
        self.pending_becomes_current();
    }

    /// Handles the 'unavailable' event sent from the wayland server
    /// This method should never be called from the client
    fn handle_unavailable(&mut self) {
        info!("handle_unavailable() was called");
        self.im.destroy();
        self.current.active = false;
        self.connector.hide_keyboard();
    }

    /// This is a helper method
    /// It moves the values of self.pending to self.current and notifies the connector, to show or hide the keyboard.
    /// It should only be called if the wayland-server or the client committed the pending changes
    fn pending_becomes_current(&mut self) {
        info!("The pending protocol state became the current state");
        let active_changed = self.current.active ^ self.pending.active;

        // Make pending changes permanent
        self.current = self.pending.clone();

        // Notify connector about changes
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

    /// This is a helper method for the delete_surrounding_text method
    /// INPUTS:
    /// before -> number of chars to delete from the surrounding_text going left from the cursor
    /// after  -> number of chars to delete from the surrounding_text going right from the cursor
    ///
    /// OUTPUTS:
    /// before (limited) -> number of chars to delete from the surrounding_text going left from the cursor  (limited)
    /// after  (limited) -> number of chars to delete from the surrounding_text going right from the cursor (limited)
    ///
    /// The wayland server ignores 'delete_surrounding_text' requests under the following conditions:
    /// A: cursor_position < before
    ///   or
    /// B: cursor_position + after > surrounding_text.len()
    /// This method limits the values of before and after to those maximums so no requests will be ignored.
    fn limit_before_after(&self, before: u32, after: u32) -> (u32, u32) {
        let cursor_position = self.pending.cursor;
        let before = if cursor_position > before {
            before
        } else {
            cursor_position
        };
        let after = if cursor_position.saturating_add(after)
            <= self.pending.surrounding_text.len().try_into().unwrap()
        {
            after
        } else {
            self.pending.surrounding_text.len() as u32 - cursor_position
        };
        (before, after)
    }

    /// This is a helper method for the delete_surrounding_text method
    /// INPUTS:
    /// before -> number of chars to delete from the surrounding_text going left from the cursor
    /// after  -> number of chars to delete from the surrounding_text going right from the cursor
    ///
    /// This method removes the amount of chars requested from self.pending.surrounding_text. This deletion not only affects the surrounding_text
    /// but also the cursor position.
    fn update_cursor_and_surrounding_text(&mut self, before: u32, after: u32) {
        let cursor_position = self.pending.cursor as usize;

        // Get str left and right of the cursor to remove the requested amount of chars from each of them
        let (string_left_of_cursor, old_string_right_of_cursor) =
            self.pending.surrounding_text.split_at(cursor_position);

        // Make a String from the reference
        let mut string_left_of_cursor = String::from(string_left_of_cursor);
        let mut string_right_of_cursor = String::from("");

        // Pop of as many chars as requested with the before parameter
        // The result is the string on the left side of the cursor for the new surrounding_text
        for _ in 0..before {
            string_left_of_cursor.pop();
        }

        // Skip as many chars as requested with the after parameter and then add all remaining chars
        // The result is the string on the right side of the cursor for the new surrounding_text
        for character in old_string_right_of_cursor.chars().skip(after as usize) {
            string_right_of_cursor.push(character);
        }

        // Get the new position of the cursor
        let new_cursor_position = string_left_of_cursor.len() as u32;

        // Join the string of the left and the right sides to make the new surrounding_text
        let mut new_surrounding_text = string_left_of_cursor;
        new_surrounding_text.push_str(&string_right_of_cursor);

        // Apply the new values of the cursor and the new surrounding_text to self
        self.pending.surrounding_text = new_surrounding_text;
        self.pending.cursor = new_cursor_position;
    }
}

#[derive(Clone, Debug)]
/// Manages the pending state and the current state of the input method.
pub struct IMService<T: 'static + KeyboardVisibility + HintPurpose> {
    im_service_arc: Arc<Mutex<IMServiceArc<T>>>, // provides an easy to use interface by hiding the Arc<Mutex<>>
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

    /// Sends a 'commit_string' request to the wayland-server
    /// INPUTS: text -> Text that will be committed
    pub fn commit_string(&self, text: String) -> Result<(), SubmitError> {
        self.im_service_arc.lock().unwrap().commit_string(text)
    }

    /// Sends a 'delete_surrounding_text' request to the wayland server
    /// INPUTS:
    /// before -> number of chars to delete from the surrounding_text going left from the cursor
    /// after  -> number of chars to delete from the surrounding_text going right from the cursor
    pub fn delete_surrounding_text(&self, before: u32, after: u32) -> Result<(), SubmitError> {
        info!("Delete surrounding text ");
        self.im_service_arc
            .lock()
            .unwrap()
            .delete_surrounding_text(before, after)
    }

    /// Sends a 'commit' request to the wayland server
    /// This makes the pending changes permanent
    pub fn commit(&self) -> Result<(), SubmitError> {
        self.im_service_arc.lock().unwrap().commit()
    }

    /// Returns if the input method is currently active
    pub fn is_active(&self) -> bool {
        self.im_service_arc.lock().unwrap().is_active()
    }

    /// Returns the current surrounding_text
    pub fn get_surrounding_text(&self) -> String {
        self.im_service_arc.lock().unwrap().get_surrounding_text()
    }
}
