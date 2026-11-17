//! Regression test for
//! <https://github.com/bytecodealliance/wasmtime/issues/14332>.
//!
//! Bindings generation with `named_imports` of an interface which has
//! unstable WIT features, where the interface is remapped through `with` to
//! bindings generated in another crate (`wasmtime-wasi` here), used to fail
//! to compile: the generated named-imports `add_to_linker*` functions read
//! the fields of the sibling module's `LinkOptions` directly, but those
//! fields are private to the crate the sibling bindings live in.
//!
//! This test primarily exists to ensure the `bindgen!` invocation below
//! compiles; the assertions additionally exercise the public `LinkOptions`
//! API of both the world and the remapped sibling module across crates.

use wasmtime::component::bindgen;
use wasmtime_wasi::p2::bindings::sockets::network as sibling;

bindgen!({
    inline: "
    package wasmtime:test;

    world test {
        import a: wasi:sockets/network@0.2.12;
    }
    ",
    path: "src/p2/wit",
    world: "wasmtime:test/test",
    with: {
        "wasi:sockets/network": wasmtime_wasi::p2::bindings::sockets::network,
    },
    named_imports: {
        "wasi:sockets/network": wasmtime_wasi::NamedId,
    },
});

#[test]
fn unstable_feature_options_accessible_across_crates() {
    let mut options = LinkOptions::default();
    assert!(!options.network_error_code_enabled());
    options.network_error_code(true);
    assert!(options.network_error_code_enabled());

    // The world-level options convert into the sibling module's options,
    // which is what the named-imports `add_to_linker` functions take.
    let sibling_options: sibling::LinkOptions = (&options).into();
    assert!(sibling_options.network_error_code_enabled());
}
