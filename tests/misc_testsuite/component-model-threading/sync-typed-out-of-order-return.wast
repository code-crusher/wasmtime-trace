;;! component_model_async = true
;;! component_model_threading = true

;; Sync-typed tasks in the same instance need not return in LIFO order. Here
;; sync-typed `s1` spawns thread T and yields to it; T calls the instance's own
;; sync-typed `s2` (via a lowered copy in a table), and `s2` yields, which
;; switches back to `s1` (the only ready thread in the instance). `s1` thus
;; returns before `s2` does. Once both have returned, no sync-typed task is in
;; progress, so the async-typed `noop` may yield and exit without trapping.
(component
  (core module $libc
    (table (export "t") 2 funcref)
    (memory (export "mem") 1))
  (core instance $libc (instantiate $libc))
  (core type $start-ty (func (param i32)))
  (core func $thread.new-indirect
    (canon thread.new-indirect $start-ty (core table $libc "t")))
  (core func $thread.yield-then-resume (canon thread.yield-then-resume))
  (core func $thread.yield (canon thread.yield))
  (core func $task.return (canon task.return))
  (core module $m
    (import "" "thread.new-indirect" (func $new (param i32 i32) (result i32)))
    (import "" "thread.yield-then-resume" (func $ytr (param i32) (result i32)))
    (import "" "thread.yield" (func $yield (result i32)))
    (import "" "task.return" (func $task.return))
    (import "libc" "t" (table 2 funcref))
    (type $v (func))
    (global $log (mut i32) (i32.const 0))
    (func $start (param i32)
      ;; call our own instance's sync-typed `s2` through the lowered import
      (call_indirect (type $v) (i32.const 1))
      (global.set $log (i32.add (global.get $log) (i32.const 100))))
    (elem (i32.const 0) func $start)
    (func (export "s1") (result i32)
      (drop (call $ytr (call $new (i32.const 0) (i32.const 0))))
      (global.set $log (i32.add (global.get $log) (i32.const 1)))
      (i32.const 7))
    (func (export "s2")
      (drop (call $yield))
      (global.set $log (i32.add (global.get $log) (i32.const 10))))
    ;; yield once (letting the leftover `s2`/T work run), then return
    (func (export "noop") (result i32)
      (i32.const 1 (; YIELD ;)))
    (func (export "cb") (param i32 i32 i32) (result i32)
      (call $task.return)
      (i32.const 0 (; EXIT ;)))
    (func (export "log") (result i32) (global.get $log))
  )
  (core instance $i (instantiate $m
    (with "" (instance
      (export "thread.new-indirect" (func $thread.new-indirect))
      (export "thread.yield-then-resume" (func $thread.yield-then-resume))
      (export "thread.yield" (func $thread.yield))
      (export "task.return" (func $task.return))))
    (with "libc" (instance $libc))))
  (func $s2 (canon lift (core func $i "s2")))
  (core func $s2-lowered (canon lower (func $s2)))
  (core module $install
    (import "libc" "t" (table 2 funcref))
    (import "" "s2" (func $s2))
    (elem (i32.const 1) func $s2))
  (core instance (instantiate $install
    (with "libc" (instance $libc))
    (with "" (instance (export "s2" (func $s2-lowered))))))
  (func (export "s1") (result u32) (canon lift (core func $i "s1")))
  (func (export "noop") async
    (canon lift (core func $i "noop") async (callback (core func $i "cb"))))
  (func (export "log") (result u32) (canon lift (core func $i "log")))
)

(assert_return (invoke "s1") (u32.const 7))
(assert_return (invoke "noop"))
(assert_return (invoke "log") (u32.const 111))
