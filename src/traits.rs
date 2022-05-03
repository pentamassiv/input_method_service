use super::SubmitError;
use wayland_client::{protocol::wl_seat::WlSeat, Main};
use wayland_protocols::unstable::text_input::v3::client::zwp_text_input_v3::{
    ContentHint, ContentPurpose,
};
use zwp_input_method::input_method_unstable_v2::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;

/// All input methods must be able to handle these functions
/// This helps write test cases, because they can be generic
pub trait InputMethod<T: IMVisibility + HintPurpose, D: ReceiveSurroundingText> {
    /// Create a new InputMethod. The connectors must implement the traits IMVisibility and HintPurpose
    fn new(
        seat: &WlSeat,
        im_manager: Main<ZwpInputMethodManagerV2>,
        ui_connector: T,
        content_connector: D,
    ) -> Self;

    /// Sends a 'commit_string' request to the wayland-server
    ///
    /// INPUTS:
    ///
    /// text -> Text that will be committed
    fn commit_string(&self, text: String) -> Result<(), SubmitError>;

    /// Sends a 'delete_surrounding_text' request to the wayland server
    ///
    /// INPUTS:
    ///
    /// before -> number of chars to delete from the surrounding_text going left from the cursor
    ///
    /// after  -> number of chars to delete from the surrounding_text going right from the cursor
    fn delete_surrounding_text(&self, before: usize, after: usize) -> Result<(), SubmitError>;

    /// Sends a 'commit' request to the wayland server
    ///
    /// This makes the pending changes permanent
    fn commit(&self) -> Result<(), SubmitError>;

    /// Returns if the input method is currently active
    fn is_active(&self) -> bool;

    /// Returns a tuple of the current strings left and right of the cursor
    fn get_surrounding_text(&self) -> (String, String);
}

/// Trait to get notified when the input method should be active or deactivated
///
/// If the user clicks for example on a text field, the method activate_im() is called
pub trait IMVisibility {
    fn activate_im(&self);
    fn deactivate_im(&self);
}

/// Trait to get notified when the text surrounding the cursor changes
pub trait ReceiveSurroundingText {
    fn text_changed(&self, string_left_of_cursor: String, string_right_of_cursor: String);
}

/// Trait to get notified when the hint or the purpose of the content changes
pub trait HintPurpose {
    fn set_hint_purpose(&self, content_hint: ContentHint, content_purpose: ContentPurpose);
}
