//! LEZ Program Registry guest. All logic is in `lezreg-program`, generated from SPEL-syntax
//! source by `spel04-shim`; this binary only wires it to the LEZ 0.4 runtime.

lee_core::actor_entrypoint!(raw lezreg_program::__lez_dispatch);
