use std::cmp;
use std::num::Wrapping;
use std::sync::{Arc, Mutex};
use wayland_client::{protocol::wl_seat::WlSeat, Filter, Main};
use wayland_protocols::misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use wayland_protocols::unstable::text_input::v3::client::zwp_text_input_v3::{
    ChangeCause, ContentHint, ContentPurpose,
};

use wayland_protocols::misc::zwp_input_method_v2::client::zwp_input_method_v2::{
    Event as InputMethodEvent, ZwpInputMethodV2,
};

use super::traits::{HintPurpose, IMVisibility, ReceiveSurroundingText};
use super::SubmitError;

// Mandatory conversion to apply filter to ZwpInputMethodV2
mod event_enum {
    use wayland_client::event_enum;
    use wayland_protocols::misc::zwp_input_method_v2::client::zwp_input_method_v2::ZwpInputMethodV2;
    event_enum!(
        Events | InputMethod => ZwpInputMethodV2
    );
}

/// Stores the state of the input method
#[derive(Clone, Debug)]
struct IMProtocolState {
    surrounding_text: String,
    cursor: usize,
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
///
/// It is called IMServiceArc and not IMService because the new() method
/// wraps IMServiceArc and returns Arc<Mutex<IMServiceArc<T>>>. This is required because it's state could get changed by multiple threads.
/// One thread could handle requests while the other one handles events from the wayland-server
pub struct IMServiceArc<
    T: 'static + IMVisibility + HintPurpose,
    D: 'static + ReceiveSurroundingText,
> {
    im: Main<ZwpInputMethodV2>,
    ui_connector: T,
    content_connector: D,
    pending: IMProtocolState,
    current: IMProtocolState,
    serial: Wrapping<u32>,
}

impl<T: IMVisibility + HintPurpose, D: ReceiveSurroundingText> IMServiceArc<T, D> {
    /// Creates a new IMServiceArc wrapped in Arc<Mutex<Self>>
    pub fn new(
        seat: &WlSeat,
        im_manager: Main<ZwpInputMethodManagerV2>,
        ui_connector: T,
        content_connector: D,
    ) -> Arc<Mutex<IMServiceArc<T, D>>> {
        // Get ZwpInputMethodV2 from ZwpInputMethodManagerV2
        let im = im_manager.get_input_method(seat);

        // Create IMServiceArc with default values
        let im_service = IMServiceArc {
            im,
            ui_connector,
            content_connector,
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
        #[cfg(feature = "debug")]
        info!("New IMService was created");
        // Return the wrapped IMServiceArc
        im_service
    }

    /// Assigns a filter to the wayland event queue to allow IMServiceArc to handle events from ZwpInputMethodV2
    pub fn assign_filter(&self, im_service: Arc<Mutex<IMServiceArc<T, D>>>) {
        let filter = Filter::new(move |event, _, _| match event {
            event_enum::Events::InputMethod { event, .. } => match event {
                InputMethodEvent::Activate => im_service.lock().unwrap().handle_activate(),
                InputMethodEvent::Deactivate => im_service.lock().unwrap().handle_deactivate(),
                InputMethodEvent::SurroundingText {
                    text,
                    cursor,
                    anchor,
                } => im_service.lock().unwrap().handle_surrounding_text(
                    text,
                    cursor as usize,
                    anchor as usize,
                ),
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
        #[cfg(feature = "debug")]
        info!("The filter was assigned to Main<ZwpInputMethodV2>");
    }

    /// Sends a 'commit_string' request to the wayland-server
    ///
    /// INPUTS: text -> Text that will be committed
    /// Wayland messages have a maximum length so the length of the text must not exceed 4000 bytes
    pub fn commit_string(&mut self, text: String) -> Result<(), SubmitError> {
        #[cfg(feature = "debug")]
        info!("Commit string '{}'", text);
        // Check if proxy is still alive. If the proxy was dead, the requests would fail silently
        match self.current.active {
            true => {
                let cursor_position = self.pending.cursor;
                // Append 'text' to the pending surrounding_text
                self.pending
                    .surrounding_text
                    .insert_str(cursor_position, &text);
                // Update the cursor
                self.pending.cursor += text.len();
                // Send the request to the wayland-server
                self.im.commit_string(text);
                Ok(())
            }
            false => Err(SubmitError::NotActive),
        }
    }

    /// Sends a 'delete_surrounding_text' request to the wayland server
    ///
    /// INPUTS:
    ///
    /// before -> number of chars to delete from the surrounding_text going left from the cursor
    ///
    /// after  -> number of chars to delete from the surrounding_text going right from the cursor
    pub fn delete_surrounding_text(
        &mut self,
        before: usize,
        after: usize,
    ) -> Result<(), SubmitError> {
        #[cfg(feature = "debug")]
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
    ///
    /// This makes the pending changes permanent
    pub fn commit(&mut self) -> Result<(), SubmitError> {
        #[cfg(feature = "debug")]
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
    pub fn is_active(&self) -> bool {
        self.current.active
    }

    /// Returns a tuple of the current strings left and right of the cursor
    pub fn get_surrounding_text(&self) -> (String, String) {
        #[cfg(feature = "debug")]
        info!("Requested surrounding text");
        let (left_str, right_str) = self.pending.surrounding_text.split_at(self.current.cursor);
        (left_str.to_string(), right_str.to_string())
    }

    /// Handles the 'activate' event sent from the wayland server
    ///
    /// This method should never be called from the client
    fn handle_activate(&mut self) {
        #[cfg(feature = "debug")]
        info!("handle_activate() was called");
        self.pending = IMProtocolState {
            active: true,
            ..IMProtocolState::default()
        };
    }

    /// Handles the 'deactivate' event sent from the wayland server
    ///
    /// This method should never be called from the client
    fn handle_deactivate(&mut self) {
        #[cfg(feature = "debug")]
        info!("handle_deactivate() was called");
        self.pending.active = false;
    }

    /// Handles the 'surrounding_text' event sent from the wayland server
    ///
    /// This method should never be called from the client
    /// The 'anchor' parameter is currently not being used
    fn handle_surrounding_text(&mut self, text: String, cursor: usize, anchor: usize) {
        #[cfg(feature = "debug")]
        info!(
            "handle_surrounding_text(text: '{}', cursor: {}) was called",
            text, cursor
        );
        self.pending.surrounding_text = text;
        self.pending.cursor = cursor;
    }

    /// Handles the 'text_change_cause' event sent from the wayland server
    ///
    /// This method should never be called from the client
    fn handle_text_change_cause(&mut self, cause: ChangeCause) {
        #[cfg(feature = "debug")]
        info!("handle_text_change_cause() was called");
        self.pending.text_change_cause = cause;
    }

    /// Handles the 'content_type' event sent from the wayland server
    ///
    /// This method should never be called from the client
    fn handle_content_type(&mut self, hint: ContentHint, purpose: ContentPurpose) {
        #[cfg(feature = "debug")]
        info!("handle_content_type() was called");
        self.pending.content_hint = hint;
        self.pending.content_purpose = purpose;
    }

    /// Handles the 'done' event sent from the wayland server
    ///
    /// This method should never be called from the client
    fn handle_done(&mut self) {
        #[cfg(feature = "debug")]
        info!("handle_done() was called");
        self.pending_becomes_current();
    }

    /// Handles the 'unavailable' event sent from the wayland server
    ///
    /// This method should never be called from the client
    fn handle_unavailable(&mut self) {
        #[cfg(feature = "debug")]
        info!("handle_unavailable() was called");
        self.im.destroy();
        self.current.active = false;
        self.ui_connector.deactivate_im();
    }

    /// This is a helper method
    ///
    /// It moves the values of self.pending to self.current and notifies the connector, to show or hide the keyboard.
    ///
    /// It should only be called if the wayland-server or the client committed the pending changes
    fn pending_becomes_current(&mut self) {
        #[cfg(feature = "debug")]
        info!("The pending protocol state became the current state");
        let active_changed = self.current.active ^ self.pending.active;
        let text_changed = self.current.surrounding_text != self.pending.surrounding_text;

        // Make pending changes permanent
        self.current = self.pending.clone();

        if text_changed {
            #[cfg(feature = "debug")]
            info!(
                "The surrounding text changed to '{}'",
                self.current.surrounding_text
            );
            let (left_str, right_str) = self.current.surrounding_text.split_at(self.current.cursor);
            let (left_str, right_str) = (left_str.to_string(), right_str.to_string());
            self.content_connector.text_changed(left_str, right_str);
        }

        // Notify connector about changes
        if active_changed {
            if self.current.active {
                self.ui_connector.activate_im();
                self.ui_connector
                    .set_hint_purpose(self.current.content_hint, self.current.content_purpose);
            } else {
                self.ui_connector.deactivate_im();
            };
        }
    }

    /// This is a helper method for the delete_surrounding_text method
    ///
    /// INPUTS:
    ///
    /// before -> number of chars to delete from the surrounding_text going left from the cursor
    ///
    /// after  -> number of chars to delete from the surrounding_text going right from the cursor
    ///
    ///
    /// OUTPUTS:
    ///
    /// before (limited) -> number of chars to delete from the surrounding_text going left from the cursor  (limited)
    ///
    /// after  (limited) -> number of chars to delete from the surrounding_text going right from the cursor (limited)
    ///
    ///
    /// The wayland server ignores 'delete_surrounding_text' requests under the following conditions:
    ///
    /// A: cursor_position < before
    ///
    ///   or
    ///
    /// B: cursor_position + after > surrounding_text.len()
    ///
    /// This method limits the values of before and after to those maximums so no requests will be ignored.
    fn limit_before_after(&self, before: usize, after: usize) -> (usize, usize) {
        let cursor_position = self.pending.cursor;
        let before = cmp::min(cursor_position, before);
        let after = cmp::min(self.pending.surrounding_text.len() - cursor_position, after);
        (before, after)
    }

    /// This is a helper method for the delete_surrounding_text method
    ///
    /// INPUTS:
    ///
    /// before -> number of chars to delete from the surrounding_text going left from the cursor
    ///
    /// after  -> number of chars to delete from the surrounding_text going right from the cursor
    ///
    /// This method removes the amount of chars requested from self.pending.surrounding_text. This deletion not only affects the surrounding_text
    /// but also the cursor position.
    fn update_cursor_and_surrounding_text(&mut self, before: usize, after: usize) {
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
        let new_cursor_position = string_left_of_cursor.len();

        // Join the string of the left and the right sides to make the new surrounding_text
        let mut new_surrounding_text = string_left_of_cursor;
        new_surrounding_text.push_str(&string_right_of_cursor);

        // Apply the new values of the cursor and the new surrounding_text to self
        self.pending.surrounding_text = new_surrounding_text;
        self.pending.cursor = new_cursor_position;
    }
}
