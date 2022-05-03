//! This crate provides an easy to use interface for the zwp_input_method_v2 protocol.
//! It allows a wayland client to serve as an input method for other wayland-clients. This could be used for virtual keyboards
//!
#[cfg(feature = "debug")]
#[macro_use]
extern crate log;

use std::sync::{Arc, Mutex};
use wayland_client::{protocol::wl_seat::WlSeat, Main};
use zwp_input_method::input_method_unstable_v2::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;

mod traits;
pub use traits::*;

use arc_input_method::*;
mod arc_input_method;

#[derive(Debug, Clone)]
/// Error when sending a request to the wayland-client
pub enum SubmitError {
    /// Input method was not activ
    NotActive,
}

#[derive(Clone, Debug)]
/// Manages the pending state and the current state of the input method.
pub struct IMService<T: 'static + IMVisibility + HintPurpose, D: 'static + ReceiveSurroundingText> {
    im_service_arc: Arc<Mutex<IMServiceArc<T, D>>>, // provides an easy to use interface by hiding the Arc<Mutex<>>
}

impl<T: IMVisibility + HintPurpose, D: ReceiveSurroundingText> InputMethod<T, D>
    for IMService<T, D>
{
    fn new(
        seat: &WlSeat,
        im_manager: Main<ZwpInputMethodManagerV2>,
        ui_connector: T,
        content_connector: D,
    ) -> Self {
        let im_service_arc = IMServiceArc::new(seat, im_manager, ui_connector, content_connector);
        IMService { im_service_arc }
    }

    fn commit_string(&self, text: String) -> Result<(), SubmitError> {
        self.im_service_arc.lock().unwrap().commit_string(text)
    }

    fn delete_surrounding_text(&self, before: usize, after: usize) -> Result<(), SubmitError> {
        self.im_service_arc
            .lock()
            .unwrap()
            .delete_surrounding_text(before, after)
    }

    fn commit(&self) -> Result<(), SubmitError> {
        self.im_service_arc.lock().unwrap().commit()
    }

    fn is_active(&self) -> bool {
        self.im_service_arc.lock().unwrap().is_active()
    }

    fn get_surrounding_text(&self) -> (String, String) {
        self.im_service_arc.lock().unwrap().get_surrounding_text()
    }
}
