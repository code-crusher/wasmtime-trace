;;! component_model_async = true
;;! component_model_threading = true

;; A sync-lowered call to an async-typed function may suspend even if the
;; callee's instance has a sync-typed task in progress, since doing so returns
;; control to the caller. That exemption must not outlive the callee's initial
;; run, nor leave the instance marked as having a sync-typed task in progress
;; after that task has returned.
;;
;; Here sync-typed `s1` spawns thread T and yields to it. T makes a
;; sync-lowered call to the instance's own async-typed `f` (via a lowered copy
;; in a table), which yields, blocking T. `s1` then returns while T is still
;; waiting for `f`, after which `f` finishes and T exits. At that point no
;; sync-typed task is in progress, so the async-typed `noop` may yield and exit
;; without trapping.
(component
  (core module $libc
    (table (export "t") 2 funcref)
    (memory (export "mem") 1))
  (core instance $libc (instantiate $libc))
  (core type $start-ty (func (param i32)))
  (core func $thread.new-indirect
    (canon thread.new-indirect $start-ty (core table $libc "t")))
  (core func $thread.yield-then-resume (canon thread.yield-then-resume))
  (core func $task.return (canon task.return))
  (core module $m
    (import "" "thread.new-indirect"
      (func $thread.new-indirect (param i32 i32) (result i32)))
    (import "" "thread.yield-then-resume"
      (func $thread.yield-then-resume (param i32) (result i32)))
    (import "" "task.return" (func $task.return))
    (import "libc" "t" (table 2 funcref))
    (type $v (func))
    (global $log (mut i32) (i32.const 0))
    (func $start (param i32)
      ;; synchronously call our own instance's async-typed `f` through the
      ;; lowered import
      (call_indirect (type $v) (i32.const 1))
      (global.set $log (i32.add (global.get $log) (i32.const 100))))
    (elem (i32.const 0) func $start)
    (func (export "s1") (result i32)
      (drop (call $thread.yield-then-resume
        (call $thread.new-indirect (i32.const 0) (i32.const 0))))
      (global.set $log (i32.add (global.get $log) (i32.const 1)))
      (i32.const 7))
    (func (export "f") (result i32)
      (i32.const 1 (; YIELD ;)))
    (func (export "f-cb") (param i32 i32 i32) (result i32)
      (global.set $log (i32.add (global.get $log) (i32.const 10)))
      (call $task.return)
      (i32.const 0 (; EXIT ;)))
    ;; yield once (letting any leftover `f`/T work run), then return
    (func (export "noop") (result i32)
      (i32.const 1 (; YIELD ;)))
    (func (export "noop-cb") (param i32 i32 i32) (result i32)
      (call $task.return)
      (i32.const 0 (; EXIT ;)))
    (func (export "log") (result i32) (global.get $log))
  )
  (core instance $i (instantiate $m
    (with "" (instance
      (export "thread.new-indirect" (func $thread.new-indirect))
      (export "thread.yield-then-resume" (func $thread.yield-then-resume))
      (export "task.return" (func $task.return))))
    (with "libc" (instance $libc))))
  (func $f async
    (canon lift (core func $i "f") async (callback (core func $i "f-cb"))))
  (core func $f-lowered (canon lower (func $f)))
  (core module $install
    (import "libc" "t" (table 2 funcref))
    (import "" "f" (func $f))
    (elem (i32.const 1) func $f))
  (core instance (instantiate $install
    (with "libc" (instance $libc))
    (with "" (instance (export "f" (func $f-lowered))))))
  (func (export "s1") (result u32) (canon lift (core func $i "s1")))
  (func (export "noop") async
    (canon lift (core func $i "noop") async
      (callback (core func $i "noop-cb"))))
  (func (export "log") (result u32) (canon lift (core func $i "log")))
)

(assert_return (invoke "s1") (u32.const 7))
(assert_return (invoke "noop"))
(assert_return (invoke "log") (u32.const 111))
