;;! bulk_memory = true

;; Active element segments whose elements are all `ref.null func` must behave
;; exactly like their spec semantics: writing null over the table range, then
;; being overwritten by later segments that overlap in order.

(module
  (table $t 10 funcref)
  (func $f (result i32) (i32.const 42))

  ;; An all-null active segment, like the ones that come up with the
  ;; function-references proposal or fuzz-generated modules, followed by an
  ;; overlapping segment that writes `$f` over one of its elements.
  (elem (table $t) (i32.const 1) func (ref.null func) (ref.null func) (ref.null func) (ref.null func))
  (elem (table $t) (i32.const 3) func $f)

  (func (export "indirect-call") (param i32) (result i32)
    local.get 0
    call_indirect $t (result i32))
)

;; Index 3 is `$f` from the second segment which overwrites the null from the
;; first. Indexes 1, 2, and 4 remain null from the first segment.
(assert_return (invoke "indirect-call" (i32.const 3)) (i32.const 42))
(assert_trap (invoke "indirect-call" (i32.const 1)) "uninitialized element")
(assert_trap (invoke "indirect-call" (i32.const 2)) "uninitialized element")
(assert_trap (invoke "indirect-call" (i32.const 4)) "uninitialized element")

;; A partially out-of-bounds segment must trap and leave no elements visible,
;; so a subsequent lookup still sees the table as it was before.
(assert_trap
  (module
    (table 10 funcref)

    (elem (i32.const 8) func (ref.null func) (ref.null func) (ref.null func))
  )
  "out of bounds"
)

;; A segment that is entirely null but only partially out of bounds similarly
;; traps rather than silently writing the in-bounds part.
(assert_trap
  (module
    (table 10 funcref)

    (elem (i32.const 9) func (ref.null func) (ref.null func) (ref.null func))
  )
  "out of bounds"
)
