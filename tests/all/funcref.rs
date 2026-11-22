use super::ref_types_module;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use wasmtime::*;

#[test]
#[cfg_attr(miri, ignore)]
fn pass_funcref_in_and_out_of_wasm() -> wasmtime::Result<()> {
    let (mut store, module) = ref_types_module(
        false,
        r#"
            (module
                (func (export "func") (param funcref) (result funcref)
                    local.get 0
                )
            )
        "#,
    )?;

    let instance = Instance::new(&mut store, &module, &[])?;
    let func = instance.get_func(&mut store, "func").unwrap();

    // Pass in a non-null funcref.
    {
        let mut results = [Val::I32(0)];
        func.call(&mut store, &[Val::FuncRef(Some(func))], &mut results)?;

        // Can't compare `Func` for equality, so this is the best we can do here.
        let result_func = results[0].unwrap_funcref().unwrap();
        assert!(FuncType::eq(&func.ty(&store), &result_func.ty(&store)));
    }

    // Pass in a null funcref.
    {
        let mut results = [Val::I32(0)];
        func.call(&mut store, &[Val::FuncRef(None)], &mut results)?;
        let result_func = results[0].unwrap_funcref();
        assert!(result_func.is_none());
    }

    // Pass in a `funcref` from another instance.
    {
        let other_instance = Instance::new(&mut store, &module, &[])?;
        let other_instance_func = other_instance.get_func(&mut store, "func").unwrap();

        let mut results = [Val::I32(0)];
        func.call(
            &mut store,
            &[Val::FuncRef(Some(other_instance_func))],
            &mut results,
        )?;
        assert_eq!(results.len(), 1);

        // Can't compare `Func` for equality, so this is the best we can do here.
        let result_func = results[0].unwrap_funcref().unwrap();
        assert!(FuncType::eq(
            &other_instance_func.ty(&store),
            &result_func.ty(&store),
        ));
    }

    // Passing in a `funcref` from another store fails.
    {
        let (mut other_store, other_module) =
            ref_types_module(false, r#"(module (func (export "f")))"#)?;
        let other_store_instance = Instance::new(&mut other_store, &other_module, &[])?;
        let f = other_store_instance
            .get_func(&mut other_store, "f")
            .unwrap();

        assert!(
            func.call(&mut store, &[Val::FuncRef(Some(f))], &mut [Val::I32(0)])
                .is_err()
        );
    }

    Ok(())
}

#[test]
fn wrong_store() -> wasmtime::Result<()> {
    let dropped = Arc::new(AtomicBool::new(false));
    {
        let mut store1 = Store::<()>::default();
        let mut store2 = Store::<()>::default();

        let set = SetOnDrop(dropped.clone());
        let f1 = Func::wrap(&mut store1, move || {
            let _ = &set;
        });
        let f2 = Func::wrap(&mut store2, move || Some(f1));
        assert!(f2.call(&mut store2, &[], &mut []).is_err());
    }
    assert!(dropped.load(SeqCst));

    return Ok(());

    struct SetOnDrop(Arc<AtomicBool>);

    impl Drop for SetOnDrop {
        fn drop(&mut self) {
            self.0.store(true, SeqCst);
        }
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn func_new_returns_wrong_store() -> wasmtime::Result<()> {
    let dropped = Arc::new(AtomicBool::new(false));
    {
        let mut store1 = Store::<()>::default();
        let mut store2 = Store::<()>::default();

        let set = SetOnDrop(dropped.clone());
        let f1 = Func::wrap(&mut store1, move || {
            let _ = &set;
        });
        let func_ty = FuncType::new(store2.engine(), None, Some(ValType::FUNCREF));
        let f2 = Func::new(&mut store2, func_ty, move |_, _, results| {
            results[0] = f1.into();
            Ok(())
        });
        assert!(f2.call(&mut store2, &[], &mut [Val::I32(0)]).is_err());
    }
    assert!(dropped.load(SeqCst));

    return Ok(());

    struct SetOnDrop(Arc<AtomicBool>);

    impl Drop for SetOnDrop {
        fn drop(&mut self) {
            self.0.store(true, SeqCst);
        }
    }
}

#[test]
#[cfg_attr(miri, ignore)]
fn call_indirect_subtypes_without_gc_store() -> wasmtime::Result<()> {
    // A `call_indirect` through a plain-`funcref` table with non-final
    // function types compiles to a runtime signature check that bottoms out
    // in the `is_subtype` libcall, and `ref.test` on typed function
    // references does the same. Neither of these instructions needs any GC
    // object, so Wasmtime does not allocate a GC heap for a module like
    // this and the libcall runs on a store with no `GcStore` at all.
    //
    // That is why the store-local subtype-check cache (see
    // `StoreGcData::subtype_check_cache`) can't be moved into the
    // `GcStore` as part of #13862: unlike `gc_roots_list`, which is only
    // touched in the middle of a collection when a `GcStore` is guaranteed
    // to exist, this cache is reached before any `GcStore` is (ever)
    // allocated. This test pins down that invariant: if the cache ever
    // moves into the `GcStore`, this test will panic with an "attempted to
    // access the store's GC heap before it has been allocated" error.
    let wat = r#"
        (module
          (type $sup (sub (func (param i32) (result i32))))
          (type $sub (sub $sup (func (param i32) (result i32))))
          (func $f (type $sub) (local.get 0))
          (table $t 1 funcref)
          (elem (table $t) (i32.const 0) func $f)
          (func (export "call") (param i32) (result i32)
            (call_indirect (type $sup) (local.get 0) (i32.const 0)))
        )
    "#;
    let mut config = Config::new();
    config.wasm_function_references(true);
    config.wasm_gc(true);
    let engine = Engine::new(&config)?;
    let module = Module::new(&engine, wat)?;
    let mut store = Store::new(&engine, ());
    assert_eq!(store.gc_heap_capacity(), 0);
    let instance = Instance::new(&mut store, &module, &[])?;
    assert_eq!(store.gc_heap_capacity(), 0);

    // The callee's type (`$sub`) differs from the call site's expected type
    // (`$sup`), so the check can't be satisfied by the fast-path equality
    // comparison and the `is_subtype` libcall actually runs here.
    let call = instance.get_typed_func::<i32, i32>(&mut store, "call")?;
    assert_eq!(call.call(&mut store, 42)?, 42);

    // The GC heap must never have been allocated by any of the above.
    assert_eq!(store.gc_heap_capacity(), 0);
    Ok(())
}
