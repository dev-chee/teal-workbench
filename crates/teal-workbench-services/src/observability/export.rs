//! 序列化前脱敏；SDK 与导出器诊断绝不进入导出路径。

use opentelemetry_sdk::{
    Resource,
    error::OTelSdkResult,
    trace::{SpanData, SpanExporter},
};
use std::time::Duration;

#[derive(Debug)]
pub(crate) struct Sanitized<E> {
    pub inner: E,
    pub policy: super::privacy::Policy,
}

impl<E: SpanExporter> SpanExporter for Sanitized<E> {
    async fn export(&self, mut batch: Vec<SpanData>) -> OTelSdkResult {
        for span in &mut batch {
            span.name = super::privacy::operation(&span.name).into();
            self.policy.attributes(&mut span.attributes);
            if matches!(span.status, opentelemetry::trace::Status::Error { .. }) {
                span.status = opentelemetry::trace::Status::error("Operation failed");
            }
            for event in &mut span.events.events {
                event.name = "event".into();
                self.policy.attributes(&mut event.attributes);
            }
            for link in &mut span.links.links {
                self.policy.attributes(&mut link.attributes);
            }
        }
        self.inner.export(batch).await
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }
}
