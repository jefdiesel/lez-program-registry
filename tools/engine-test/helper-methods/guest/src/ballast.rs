// Shared by the `lezreg_ballast_*` guests: a program that keeps its state and reads one byte of
// an embedded blob, so the blob stays in the binary. Only its size matters (CYCLES.md: the
// loader charges per byte it reads when a program of this size is deployed).
use lee_core::program::{ReceiveInput, Response};

fn handle(input: &ReceiveInput, _message: Vec<u8>) -> Response {
    let i = input.message.len() % BALLAST.len();
    // SAFETY: an in-bounds read of a static; volatile so the whole blob is kept.
    let b = unsafe { core::ptr::read_volatile(&BALLAST[i]) };
    core::hint::black_box(b);
    Response::keep_state()
}

lee_core::actor_entrypoint!(handle);
