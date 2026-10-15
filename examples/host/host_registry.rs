//! 将 Teal 声明连接到内存编辑器，演示精确签名、默认拒绝和显式能力授权。
//!
//! 运行 `cargo run -p teal --example host_registry`。示例只修改内存字符串；
//! 真正访问路径的宿主还必须在回调前执行对应的动态资源授权。

use std::sync::{Arc, Mutex};
use teal_compiler::{CompileOptions, RuntimeTier, Source};
use teal_host::{HostFunctionDefinition, HostRegistry, NativeHostRuntime};
use teal_runtime::HostExecutionGrant as ExecutionGrant;
use teal_runtime::{FsSourceLoader, RunPhase, compile_source, execute_program_with_audit};
use teal_vm::{ExternalCallRecovery, HostCallSignature, HostError, HostValue, RuntimeType};

#[derive(Default)]
struct ExampleAuditSink;

impl teal_security::AuditSink for ExampleAuditSink {
    fn append(
        &self,
        _: &teal_security::Event,
    ) -> Result<teal_security::Receipt, teal_security::AuditError> {
        Ok(teal_security::Receipt {
            sequence: 1,
            hash: format!("{:064x}", 1),
        })
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let document = Arc::new(Mutex::new("Hello from the host".to_owned()));
    let applied = Arc::new(Mutex::new(false));
    let mut registry = HostRegistry::new();
    let read_document = document.clone();
    registry.register(HostFunctionDefinition::new(
        "rein.editor",
        "read",
        HostCallSignature::new([RuntimeType::Int], RuntimeType::String)?,
        "editor.read",
        ExternalCallRecovery::Unsupported,
        move |arguments| match arguments {
            [HostValue::Int(0)] => Ok(HostValue::String(read_document.lock().unwrap().clone())),
            _ => Err(HostError::new(
                std::io::ErrorKind::InvalidInput,
                "Unknown document",
            )),
        },
    ))?;
    let write_document = document.clone();
    let did_apply = applied.clone();
    registry.register(HostFunctionDefinition::new(
        "rein.editor",
        "apply",
        HostCallSignature::new([RuntimeType::String, RuntimeType::Bool], RuntimeType::None)?,
        "editor.write",
        ExternalCallRecovery::Unsupported,
        move |arguments| match arguments {
            [HostValue::String(text), HostValue::Bool(true)] => {
                *write_document.lock().unwrap() = text.clone();
                *did_apply.lock().unwrap() = true;
                Ok(HostValue::None)
            }
            _ => Err(HostError::new(
                std::io::ErrorKind::InvalidInput,
                "Expected confirmed text",
            )),
        },
    ))?;
    let program = compile_source(
        Source::new(
            "safe_host_declarations.tl",
            include_str!("safe_host_declarations.tl"),
        ),
        &FsSourceLoader,
        CompileOptions::default().with_tier(RuntimeTier::Agentic),
    )?;
    // 同一份策略绑定到真实 HostRuntime；声明函数并不等于授予调用权限。
    let mut denied = NativeHostRuntime::builder()
        .trusted()
        .registry(registry.clone())
        .authorization(Arc::new(ExecutionGrant::deny_all("example/denied/v1")))
        .build()?;
    let audit = teal_security::AuditContext::new(Arc::new(ExampleAuditSink));
    let failure = execute_program_with_audit(
        program.clone(),
        &mut denied,
        teal_runtime::VmOptions::for_tier(RuntimeTier::Agentic),
        audit.clone(),
    )
    .expect_err("Missing grant must fail");
    assert_eq!(failure.phase, RunPhase::Load);
    assert!(!*applied.lock().unwrap());

    let grant = ExecutionGrant::deny_all("example/editor/v1")
        .allow_permission("editor.read")
        .allow_permission("editor.write");
    let mut host = NativeHostRuntime::builder()
        .trusted()
        .registry(registry)
        .authorization(Arc::new(grant))
        .build()?;
    execute_program_with_audit(
        program,
        &mut host,
        teal_runtime::VmOptions::for_tier(RuntimeTier::Agentic),
        audit,
    )?;
    assert!(*applied.lock().unwrap());
    assert_eq!(*document.lock().unwrap(), "Hello from the host");
    println!("Host calls denied without grants and executed with grants");
    Ok(())
}

#[test]
fn host_example_checks_denial_and_success() {
    main().expect("Host example should verify both authorization paths");
}
