;; A `wasi:http/proxy`-shaped component which imports `wasi:cli/environment`
;; under the label `env2` and exports `wasi:http/incoming-handler` under the
;; label `h`. `wasmtime serve` must handle both labelled imports and exports
;; for this component to instantiate.
(component
  (import "wasi:http/types@0.2.12" (instance $types
    (export "incoming-request" (type $ir (sub resource)))
    (export "response-outparam" (type $rop (sub resource)))
  ))
  (alias export $types "incoming-request" (type $ir))
  (alias export $types "response-outparam" (type $rop))

  (import "env2" (implements "wasi:cli/environment@0.2.12") (instance $env
    (export "get-environment" (func (result (list (tuple string string)))))
  ))

  (core module $m
    (func (export "handle") (param i32 i32))
  )
  (core instance $i (instantiate $m))

  (func $handle
    (param "request" (own $ir))
    (param "response-out" (own $rop))
    (canon lift (core func $i "handle")))

  (instance $inst (export "handle" (func $handle)))
  (export "h" (implements "wasi:http/incoming-handler@0.2.12") (instance $inst))
)
