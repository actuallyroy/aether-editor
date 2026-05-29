// Phase-4 extension runtime: a QuickJS sandbox for executing VSCode extension
// JavaScript. This is the foundation — currently just proves the engine embeds
// and runs JS in-process. The `vscode` API shim + CommonJS loader come next.

use rquickjs::{Context, Runtime};

/// Smoke test: evaluate a trivial JS expression to confirm QuickJS is embedded
/// and working. Returns the result, or None on failure.
pub fn smoke_eval() -> Option<i64> {
    let rt = Runtime::new().ok()?;
    let ctx = Context::full(&rt).ok()?;
    ctx.with(|ctx| ctx.eval::<i64, _>("1 + 2 * 20").ok())
}

#[cfg(test)]
mod tests {
    #[test]
    fn quickjs_runs() {
        assert_eq!(super::smoke_eval(), Some(41));
    }
}
