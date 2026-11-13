;; This component imports `wasi:cli/environment` under the label `env2` rather
;; than its interface name and additionally exports `wasi:cli/run` under the
;; label `h`. Both require the CLI to handle labelled imports and exports.
(component
  (import "env2" (implements "wasi:cli/environment@0.2.0") (instance $env
    (export "get-arguments" (func (result (list string))))))

  (alias export $env "get-arguments" (func $get-arguments))

  (core module $libc
    (memory (export "mem") 1)
    (func (export "realloc") (param i32 i32 i32 i32) (result i32) i32.const 8)
  )
  (core instance $libc (instantiate $libc))
  (alias core export $libc "mem" (core memory $mem))
  (alias core export $libc "realloc" (core func $realloc))

  (core func $get-arguments-l
    (canon lower (func $get-arguments) (memory $mem) (realloc (core func $realloc))))

  (core module $m
    (import "" "get-arguments" (func $get_arguments (param i32)))
    (memory (export "memory") 1)
    (func (export "run") (result i32)
      ;; Fetch the arguments and then ignore them, just making sure the
      ;; labelled import is actually linked and callable.
      (call $get_arguments (i32.const 0))
      i32.const 0)
  )
  (core instance $i (instantiate $m (with "" (instance
    (export "get-arguments" (func $get-arguments-l))))))

  (func $run (result (result))
    (canon lift (core func $i "run")))

  (instance $inst (export "run" (func $run)))
  (export "h" (implements "wasi:cli/run@0.2.0") (instance $inst))
)
