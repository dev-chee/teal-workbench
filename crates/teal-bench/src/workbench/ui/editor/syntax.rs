//! 编辑器语法工作线程与有界嵌入语言捕获。
//!
//! 容量为一的通道合并过期工作，界面保留最新待发送快照；完成结果仍须再次核对修订。
//! 关闭发送端和结果接收端后再等待线程，避免结果队列已满造成退出阻塞。

use teal_workbench_core::{BufferHandle, DocumentChange};
use teal_workbench_core::syntax::{
    Capture, GrammarSpec, HighlightQuery, InjectionSpec, InputEdit, Parser, ParserTable, SyntaxTree,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};

type Job = (u64, String, tracing::Span, tracing::Dispatch);
type Completed = (
    u64,
    SyntaxTree,
    Vec<Capture>,
    tracing::Span,
    tracing::Dispatch,
);

pub(super) struct SyntaxState {
    worker: std::thread::JoinHandle<()>,
    jobs: SyncSender<Job>,
    results: Receiver<Completed>,
    /// 最近观察到的缓冲区修订，不等同于工作线程已经接收的修订。
    requested: Option<u64>,
    /// 队列满时在界面侧保留最新快照，下一次轮询继续尝试发送。
    pending: Option<Job>,
    pub(super) captures: Vec<Capture>,
    applied: Option<u64>,
}

impl SyntaxState {
    pub(super) fn new(
        parser: Parser,
        query: HighlightQuery,
        injections: Vec<InjectionSpec>,
        languages: Vec<(GrammarSpec, ParserTable)>,
    ) -> Self {
        let (jobs, input) = mpsc::sync_channel::<Job>(1);
        let (output, results) = mpsc::sync_channel(1);

        let worker = std::thread::spawn(move || {
            let languages = languages
                .into_iter()
                .filter_map(|(grammar, table)| {
                    Parser::with_table(&grammar, table).ok().map(|parser| {
                        (
                            grammar.language.clone(),
                            (
                                parser,
                                HighlightQuery::from_grammar(&grammar),
                                grammar.injections,
                            ),
                        )
                    })
                })
                .collect::<BTreeMap<_, _>>();

            let mut tree = parser.parse("");
            while let Ok(mut job) = input.recv() {
                // 重新分析最新快照，不将过期的中间任务排队。
                while let Ok(newer) = input.try_recv() {
                    job = newer;
                }

                let captures = tracing::dispatcher::with_default(&job.3, || {
                    job.2.in_scope(|| {
                        let delta = DocumentChange::between(tree.source(), &job.1, 0, job.0);
                        tree = parser.reparse(
                            &tree,
                            &InputEdit {
                                old_range: delta.old_byte_range,
                                inserted: delta.inserted,
                            },
                        );
                        let mut captures = query.evaluate(&tree);
                        injected_captures(&tree, &injections, &languages, &mut captures);
                        job.2.record("operation.result", "ok");
                        tracing::info!(event.name = "completed");
                        captures
                    })
                });
                if output
                    .send((job.0, tree.clone(), captures, job.2, job.3))
                    .is_err()
                {
                    break;
                }
            }
        });

        Self {
            worker,
            jobs,
            results,
            requested: None,
            pending: None,
            captures: Vec::new(),
            applied: None,
        }
    }

    pub(super) fn stop(self) -> std::thread::JoinHandle<()> {
        // 关闭两端也会释放正在等待已满结果队列的工作线程。
        drop(self.jobs);
        drop(self.results);
        self.worker
    }

    pub(super) fn update(&mut self, buffer: &BufferHandle) {
        let revision = buffer.revision();
        if self.requested != Some(revision) {
            self.requested = Some(revision);
            self.pending = Some((
                revision,
                buffer.to_string(),
                tracing::info_span!(
                    "teal.bench.syntax",
                    buffer.revision = revision,
                    operation.result = tracing::field::Empty
                ),
                tracing::dispatcher::get_default(Clone::clone),
            ));
            self.captures.clear();
        }

        // 完成任务可能落后于输入；绝不显示其他修订的捕获。
        while let Ok((version, _, captures, span, dispatch)) = self.results.try_recv() {
            if version == revision {
                tracing::dispatcher::with_default(&dispatch, || {
                    span.in_scope(|| {
                        self.captures = captures;
                        self.applied = Some(version);
                    });
                });
            }
        }

        // 发送使用非阻塞尝试；保留未发送任务以施加背压，而不是卡住终端事件循环。
        if let Some(job) = self.pending.take()
            && let Err(TrySendError::Full(job)) = self.jobs.try_send(job)
        {
            self.pending = Some(job);
        }
    }
}

type Languages = BTreeMap<String, (Parser, HighlightQuery, Vec<InjectionSpec>)>;

fn injected_captures(
    tree: &SyntaxTree,
    rules: &[InjectionSpec],
    languages: &Languages,
    captures: &mut Vec<Capture>,
) {
    let mut stack = vec![(tree.clone(), rules, 0_usize, 1_u16)];
    let mut visited = BTreeSet::new();
    let mut bytes = 0_usize;

    // 同时限制嵌套和总工作量，包含文法注入自身的情况。
    while let Some((tree, rules, offset, depth)) = stack.pop() {
        if depth > 32 {
            continue;
        }
        for node in tree.root().walk() {
            for rule in rules.iter().filter(|rule| rule.node_kind == node.kind()) {
                let range = node.range();
                let Some(source) = tree.source().get(range.clone()) else {
                    continue;
                };
                let Some((parser, query, nested)) = languages.get(&rule.language) else {
                    continue;
                };
                let start = offset + range.start;
                if source.is_empty()
                    || !visited.insert((rule.language.as_str(), start, start + source.len()))
                {
                    continue;
                }
                bytes = bytes.saturating_add(source.len());
                if bytes > 8 * 1024 * 1024 || visited.len() > 2048 {
                    return;
                }
                let embedded = parser.parse(source);
                captures.extend(query.evaluate(&embedded).into_iter().map(|mut capture| {
                    capture.range = capture.range.start + start..capture.range.end + start;
                    capture.priority = depth;
                    capture
                }));
                stack.push((embedded, nested.as_slice(), start, depth + 1));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use teal_workbench_core::syntax::GrammarBuilder;
    #[test]
    fn public_editor_shutdown_releases_its_worker_and_pending_contexts() {
        let mut grammar = GrammarBuilder::new("shutdown");
        grammar
            .token("word", "[a-z]+", 1)
            .production("Root", ["word"], "root")
            .start("Root");
        let grammar = grammar.finish().unwrap();
        let mut editor = super::super::EditorPanel::new(
            teal_workbench_core::Document::from_text("hello"),
            teal_workbench_core::KeymapPreset::VimFirst,
        );
        editor.set_grammar(&grammar).unwrap();
        let worker = editor.stop_syntax().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !worker.is_finished() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        worker.join().unwrap();
        assert!(editor.stop_syntax().is_none());
    }
    #[test]
    fn worker_reparses_latest_unicode_revision_and_bounds_injection_cycles() {
        let mut host = GrammarBuilder::new("host");
        host.token("prefix", "\u{524d}", 1)
            .token("body", "[a-z]+", 1)
            .production("Root", ["prefix", "body"], "root")
            .start("Root")
            .injection("body", "embedded");
        let host = host.finish().unwrap();
        let mut embedded = GrammarBuilder::new("embedded");
        embedded
            .token("word", "[a-z]+", 1)
            .production("Root", ["word"], "root")
            .start("Root")
            .capture("word", "keyword")
            .injection("word", "embedded");
        let embedded = embedded.finish().unwrap();
        let table = ParserTable::build(&embedded).unwrap();
        let mut syntax = SyntaxState::new(
            Parser::new(&host).unwrap(),
            HighlightQuery::from_grammar(&host),
            host.injections,
            vec![(embedded, table)],
        );
        let buffer = BufferHandle::new(teal_workbench_core::Document::from_text("\u{524d}abc"));
        syntax.update(&buffer);
        buffer
            .apply(teal_workbench_core::Transaction::insert(2, "z"))
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while syntax.applied != Some(buffer.revision()) {
            assert!(std::time::Instant::now() < deadline);
            syntax.update(&buffer);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(syntax.captures.len(), 1);
        assert_eq!(syntax.captures[0].range, 3..7);
        assert_eq!(syntax.captures[0].priority, 1);
    }
}
