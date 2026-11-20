;;! gc = true

;; Counterpart to `tests/disas/component-model/known-imported-global-value.wat`.
;;
;; `$N`'s import of `$m1`'s immutable global is always satisfied by that same
;; defined global, so `$N` can inline the global's constant value at each
;; `global.get`. That is only the first dimension of static knowledge about the
;; import, however: `$O` is instantiated with the exports of two different
;; instances of `$M`, so it cannot know which global it gets, which makes
;; `$M`'s exported globals ambiguous. Therefore the second dimension of static
;; knowledge does not hold, and every access of those globals must use the
;; conservative `PublicGlobal` alias region -- but inlining the immutable
;; global's value is independent of alias regions and still fine.
;;
;; When inlining is enabled (randomly by the wast file fuzzer) the inlined
;; constant must still agree with the actual value of the global, and the
;; mutable global probes must not see stale values from mismatched alias
;; regions.

(component
  (core module $M
    (global (export "imm") i32 (i32.const 42))
    (global (export "mut") (mut i32) (i32.const 0))

    (func (export "set-mut")
      (global.set 1 (i32.const 0x1234)))
  )
  (core instance $m1 (instantiate $M))
  (core instance $m2 (instantiate $M))

  (core module $N
    (import "" "imm" (global $imm i32))
    (import "" "mut" (global $mut (mut i32)))
    (import "" "set-mut" (func $set-mut))

    ;; The import's value is always the constant it was initialized with, so
    ;; this can be inlined.
    (func (export "get-imm") (result i32)
      (global.get $imm))

    ;; The mutable import, on the other hand, must be loaded through the
    ;; conservative public alias region, like every other access of this
    ;; global in this component.
    (func (export "probe-mut") (result i32)
      (global.set $mut (i32.const 1))
      (call $set-mut)
      (global.get $mut))
  )
  (core instance $n (instantiate $N (with "" (instance $m1))))

  (core module $O
    (import "" "imm" (global $imm i32))
    (import "" "mut" (global $mut (mut i32)))

    ;; `$O` is instantiated twice, with `$m1`'s and `$m2`'s exports
    ;; respectively, so it cannot statically know which globals its imports
    ;; are, and this must really load the global.
    (func (export "get-imm") (result i32)
      (global.get $imm))
  )
  (core instance $o1 (instantiate $O (with "" (instance $m1))))
  (core instance $o2 (instantiate $O (with "" (instance $m2))))

  (func (export "n-get-imm") (result u32)
    (canon lift (core func $n "get-imm")))
  (func (export "n-probe-mut") (result u32)
    (canon lift (core func $n "probe-mut")))
  (func (export "o1-get-imm") (result u32)
    (canon lift (core func $o1 "get-imm")))
  (func (export "o2-get-imm") (result u32)
    (canon lift (core func $o2 "get-imm")))
)

(assert_return (invoke "n-get-imm") (u32.const 42))
(assert_return (invoke "n-probe-mut") (u32.const 0x1234))
(assert_return (invoke "o1-get-imm") (u32.const 42))
(assert_return (invoke "o2-get-imm") (u32.const 42))
