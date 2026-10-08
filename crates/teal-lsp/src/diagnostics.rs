//! 编译诊断、来源聚合与跨文档发布状态。

use super::*;

/// 一份针对生产来源的诊断性出版物。
#[derive(Clone, Debug)]
pub(super) struct DiagnosticReport {
    pub(super) publications: Vec<(String, Vec<Value>)>,
}

/// 按分析根保留上一轮诊断，并为受影响 URI 重新聚合当前仍成立的结果。
#[derive(Default)]
pub(super) struct DiagnosticTracker {
    pub(super) reports: HashMap<String, DiagnosticReport>,
}

impl DiagnosticTracker {
    /// 更新一个分析根，并同时清空它不再产生的跨文档诊断。
    pub(super) fn update(
        &mut self,
        requested_uri: &str,
        report: DiagnosticReport,
    ) -> DiagnosticReport {
        let mut affected = BTreeSet::from([requested_uri.to_owned()]);
        if let Some(previous) = self.reports.insert(requested_uri.to_owned(), report) {
            affected.extend(previous.publications.into_iter().map(|(uri, _)| uri));
        }
        if let Some(current) = self.reports.get(requested_uri) {
            affected.extend(current.publications.iter().map(|(uri, _)| uri.clone()));
        }
        self.aggregate(affected)
    }

    /// 删除一个分析根，并重新发布其他根仍然拥有的诊断。
    pub(super) fn remove(&mut self, requested_uri: &str) -> DiagnosticReport {
        let mut affected = BTreeSet::from([requested_uri.to_owned()]);
        if let Some(previous) = self.reports.remove(requested_uri) {
            affected.extend(previous.publications.into_iter().map(|(uri, _)| uri));
        }
        self.aggregate(affected)
    }

    /// 合并所有根对一组 URI 的当前诊断；相同诊断只发布一次。
    pub(super) fn aggregate(&self, affected: BTreeSet<String>) -> DiagnosticReport {
        let publications = affected
            .into_iter()
            .map(|uri| {
                let mut diagnostics = Vec::new();
                for report in self.reports.values() {
                    for (_, values) in report
                        .publications
                        .iter()
                        .filter(|(target, _)| target == &uri)
                    {
                        for value in values {
                            if !diagnostics.contains(value) {
                                diagnostics.push(value.clone());
                            }
                        }
                    }
                }
                (uri, diagnostics)
            })
            .collect();
        DiagnosticReport { publications }
    }
}

impl ServerState {
    /// 原子替换所有受影响根的诊断后才发布，避免其他根暂时把旧错误重新聚合出来。
    pub(super) fn refresh_diagnostics(
        &mut self,
        uri: &str,
        tracker: &mut DiagnosticTracker,
    ) -> DiagnosticReport {
        let changed = document_uri_and_path(uri).map(|(_, path)| normalize_path(path));
        let mut roots = BTreeSet::from([uri.to_owned()]);
        if let Some(path) = &changed {
            roots.extend(
                self.dependencies
                    .iter()
                    .filter(|(_, inputs)| inputs.contains(path))
                    .map(|(root, _)| root.clone()),
            );
        }
        let affected_paths = roots
            .iter()
            .filter_map(|root| document_uri_and_path(root).map(|(_, path)| normalize_path(path)))
            .collect::<BTreeSet<_>>();
        for document in self.documents.values_mut() {
            if document
                .resolved_compilation
                .as_ref()
                .is_some_and(|(_, _, analysis)| {
                    analysis.sources().iter().any(|source| {
                        affected_paths
                            .contains(&normalize_path(PathBuf::from(source.source_name())))
                    })
                })
            {
                document.dependencies_valid = false;
            }
        }
        let mut affected = BTreeSet::from([uri.to_owned()]);
        for root in roots {
            let updated = if self.documents.contains_key(&root) {
                let report = self.diagnostic_report(&root);
                tracker.update(&root, report)
            } else {
                self.dependencies.remove(&root);
                tracker.remove(&root)
            };
            affected.extend(updated.publications.into_iter().map(|(uri, _)| uri));
        }
        tracker.aggregate(affected)
    }

    pub(super) fn diagnostic_report(&mut self, uri: &str) -> DiagnosticReport {
        let Some(document) = self.documents.get(uri) else {
            return DiagnosticReport {
                publications: vec![(uri.to_owned(), Vec::new())],
            };
        };
        let document_path = document.path.clone();
        let document_text = document.text.clone();
        let document_index = document.index.clone();
        self.dependencies
            .insert(uri.to_owned(), BTreeSet::from([document_path.clone()]));
        if let Err(error) = package_catalog() {
            return DiagnosticReport {
                publications: vec![(
                    uri.to_owned(),
                    vec![json!({
                        "range": document_index.range(Span::new(0, 0)),
                        "severity": 1,
                        "source": "teal",
                        "code": "teal::package-catalog",
                        "message": error.message.clone()
                    })],
                )],
            };
        }
        let source = Source::new(document_path.to_string_lossy(), document_text.to_string());
        let module = match parse(&source) {
            Ok(module) => module,
            Err(error) => {
                return DiagnosticReport {
                    publications: vec![(
                        uri.to_owned(),
                        vec![json!({
                            "range": document_index.range(error.span),
                            "severity": 1,
                            "source": "teal",
                            "code": teal_compiler::CompileErrorKind::Parse.code(),
                            "message": error.message
                        })],
                    )],
                };
            }
        };

        let configured = self
            .entry_files
            .iter()
            .any(|entry| normalize_path(entry.clone()) == document_path);
        let declares_main = module
            .items
            .iter()
            .any(|item| matches!(item, Item::Function(function) if function.name == "main"));
        if !configured && !declares_main {
            return DiagnosticReport {
                publications: vec![(uri.to_owned(), Vec::new())],
            };
        }
        let options =
            CompileOptions::default().with_runtime_profile(teal_compiler::RuntimeProfile::agent());
        let loader = self.loader();
        let result = compile_with_analysis(source, &loader, options);
        self.dependencies
            .entry(uri.to_owned())
            .or_default()
            .extend(loader.reads.into_inner());
        match result {
            Ok(output) => {
                self.apply_compilation_analysis(Arc::new(output.analysis));
                let mut publications = HashMap::<String, Vec<Value>>::new();
                publications.entry(uri.to_owned()).or_default();
                for warning in output.warnings {
                    let (target_uri, target_index) = self.diagnostic_target(
                        warning.source_name.as_deref(),
                        warning.source_code.as_deref(),
                        uri,
                        &document_index,
                    );
                    let span = warning
                        .span
                        .map(|span| {
                            Span::new(
                                u32::try_from(span.offset()).unwrap_or(0),
                                u32::try_from(span.len()).unwrap_or(0),
                            )
                        })
                        .unwrap_or(Span::new(0, 0));
                    publications.entry(target_uri).or_default().push(json!({
                        "range": target_index.range(span),
                        "severity": 2,
                        "source": "teal",
                        "code": warning.lint.name(),
                        "message": warning.message,
                    }));
                }
                DiagnosticReport {
                    publications: publications.into_iter().collect(),
                }
            }
            Err(error) => {
                let (target_uri, target_index) = self.diagnostic_target(
                    error.source_name.as_deref(),
                    error.source_code.as_deref(),
                    uri,
                    &document_index,
                );
                let span = error.span.map(|span| {
                    let start = u32::try_from(span.offset()).unwrap_or(0);
                    let len = u32::try_from(span.len()).unwrap_or(0);
                    Span::new(start, len)
                });
                let mut publications = HashMap::<String, Vec<Value>>::new();
                publications.entry(target_uri).or_default().push(json!({
                        "range": target_index.range(span.unwrap_or(Span::new(0, 0))),
                        "severity": 1,
                        "source": "teal",
                        "code": error.diagnostic_code(),
                        "message": error.message
                }));
                for related in error.related {
                    let (target_uri, target_index) = self.diagnostic_target(
                        related.source_name.as_deref(),
                        related.source_code.as_deref(),
                        uri,
                        &document_index,
                    );
                    let span = related
                        .span
                        .map(|span| {
                            Span::new(
                                u32::try_from(span.offset()).unwrap_or(0),
                                u32::try_from(span.len()).unwrap_or(0),
                            )
                        })
                        .unwrap_or(Span::new(0, 0));
                    publications.entry(target_uri).or_default().push(json!({
                        "range": target_index.range(span),
                        "severity": 1,
                        "source": "teal",
                        "code": related.lint.name(),
                        "message": related.message,
                    }));
                }
                DiagnosticReport {
                    publications: publications.into_iter().collect(),
                }
            }
        }
    }

    /// 只返回直接单位一级检查的诊断物品。
    #[cfg(test)]
    pub(super) fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
        self.diagnostic_report(uri)
            .publications
            .into_iter()
            .flat_map(|(_, diagnostics)| diagnostics)
            .collect()
    }

    pub(super) fn diagnostic_target(
        &self,
        source_name: Option<&str>,
        source_code: Option<&str>,
        fallback_uri: &str,
        fallback_index: &LineIndex,
    ) -> (String, LineIndex) {
        if let Some(source_name) = source_name {
            let error_path = normalize_path(PathBuf::from(source_name));
            if let Some(candidate) = self
                .documents
                .values()
                .find(|candidate| candidate.path == error_path)
            {
                return (candidate.uri.clone(), candidate.index.clone());
            }
            if let Some(source_code) = source_code {
                return (
                    path_to_uri(Path::new(source_name)),
                    LineIndex::new(source_code.to_owned()),
                );
            }
        }
        (fallback_uri.to_owned(), fallback_index.clone())
    }
}
