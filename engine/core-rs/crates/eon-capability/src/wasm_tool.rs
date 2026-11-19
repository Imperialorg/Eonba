//! WebAssembly tool execution with fuel metering and no host imports (docs/implementation/01, section 5, sandbox layer).
//!
//! A tool module runs under a fixed fuel budget. Running out of fuel is reported as `ResourceExhausted`. The linker is
//! empty, so a module that declares any import fails to instantiate, which means it gets no WASI, no filesystem, no
//! network, and no clock.

use std::sync::OnceLock;

use eon_primitives::{EonError, EonResult};
use wasmtime::{Config, Engine, Linker, Module, Store, Trap};

fn operation_failed(operation: &str, reason: impl std::fmt::Display) -> EonError {
    EonError::OperationFailed {
        operation: operation.to_owned(),
        reason: reason.to_string(),
    }
}

fn engine() -> EonResult<&'static Engine> {
    static ENGINE: OnceLock<Result<Engine, String>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            let mut config = Config::new();
            config.consume_fuel(true);
            Engine::new(&config).map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| operation_failed("wasm.engine", e))
}

/// Compiles `module` (binary or WAT text), instantiates it with no imports, and calls its exported `(i32) -> i32`
/// function `export` with `arg` under a budget of `fuel` instructions.
pub fn call_i32_export(module: &[u8], export: &str, arg: i32, fuel: u64) -> EonResult<i32> {
    let engine = engine()?;
    let module = Module::new(engine, module).map_err(|e| operation_failed("wasm.compile", e))?;
    let mut store = Store::new(engine, ());
    store.set_fuel(fuel).map_err(|e| operation_failed("wasm.fuel", e))?;
    let linker: Linker<()> = Linker::new(engine);
    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(|e| operation_failed("wasm.instantiate", format!("{e:?}")))?;
    let func = instance
        .get_typed_func::<i32, i32>(&mut store, export)
        .map_err(|e| operation_failed("wasm.export", e))?;
    func.call(&mut store, arg).map_err(|e| {
        if matches!(e.downcast_ref::<Trap>(), Some(Trap::OutOfFuel)) {
            EonError::ResourceExhausted {
                resource: "wasm_fuel".into(),
                requested: fuel,
                available: 0,
            }
        } else {
            operation_failed("wasm.call", e)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADD_TWO: &str = r#"(module (func (export "run") (param i32) (result i32) local.get 0 i32.const 2 i32.add))"#;
    const SPIN: &str = r#"(module (func (export "run") (param i32) (result i32) (loop $l (br $l)) i32.const 0))"#;
    const WITH_IMPORT: &str = r#"(module
        (import "env" "exfiltrate" (func $x))
        (func (export "run") (param i32) (result i32) call $x local.get 0))"#;

    #[test]
    fn a_pure_tool_runs_and_returns_its_result() {
        assert_eq!(call_i32_export(ADD_TWO.as_bytes(), "run", 40, 1_000_000).unwrap(), 42);
    }

    #[test]
    fn an_endless_loop_is_stopped_by_the_fuel_budget() {
        let outcome = call_i32_export(SPIN.as_bytes(), "run", 0, 50_000);
        assert!(
            matches!(outcome, Err(EonError::ResourceExhausted { ref resource, .. }) if resource == "wasm_fuel"),
            "{outcome:?}"
        );
    }

    #[test]
    fn a_module_that_declares_a_host_import_cannot_instantiate() {
        let outcome = call_i32_export(WITH_IMPORT.as_bytes(), "run", 0, 1_000_000);
        assert!(matches!(outcome, Err(EonError::OperationFailed { ref operation, .. }) if operation == "wasm.instantiate"));
    }

    #[test]
    fn a_missing_export_is_reported_not_guessed() {
        assert!(matches!(
            call_i32_export(ADD_TWO.as_bytes(), "nope", 1, 1_000_000),
            Err(EonError::OperationFailed { ref operation, .. }) if operation == "wasm.export"
        ));
    }
}
