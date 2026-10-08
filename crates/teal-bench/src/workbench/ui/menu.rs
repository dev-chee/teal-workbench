//! 通用模糊菜单原语。
//!
//! 补全、picker、Debug 变量/栈、模型与权限切换共用同一套选择语义：调用方提供带标签与
//! 稳定值的条目，菜单负责过滤、移动与确认。菜单不渲染边框，也不承担应用策略。

use teal_workbench_core::{InputEvent, Key};

use super::MenuItem;

/// 补全菜单对一次按键的处理结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionOutcome {
    /// 菜单没有处理该按键。
    Ignored,

    /// 菜单改变了选择或消费了该按键。
    Consumed,

    /// 用户接受了一个补全候选，应写回输入。
    Accepted(String),
}

/// 补全候选菜单。
///
/// 与 [`Menu`] 的差别是它服务于正在编辑的输入前缀：候选由调用方按当前前缀重建，
/// 菜单只维护选中位置并提供键盘导航与接受动作。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompletionMenu {
    candidates: Vec<String>,
    selected: usize,
}

impl CompletionMenu {
    /// 用新的候选集合重建菜单，并把选择重置到第一个可用候选。
    pub fn set_candidates(&mut self, candidates: Vec<String>) {
        self.candidates = candidates;
        self.selected = 0;
    }

    /// 清空候选。
    pub fn clear(&mut self) {
        self.candidates.clear();
        self.selected = 0;
    }

    /// 返回当前候选。
    #[must_use]
    pub fn candidates(&self) -> &[String] {
        &self.candidates
    }

    /// 返回是否没有候选。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    /// 返回当前选中下标。
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// 返回当前选中的候选。
    #[must_use]
    pub fn current(&self) -> Option<&str> {
        self.candidates.get(self.selected).map(String::as_str)
    }

    /// 处理一次按键；只有存在候选时才消费导航键。
    pub fn handle_key(&mut self, event: InputEvent) -> CompletionOutcome {
        if self.candidates.is_empty() {
            return CompletionOutcome::Ignored;
        }
        let backwards = event.modifiers.shift;
        match event.key {
            Key::Down => {
                self.selected = (self.selected + 1) % self.candidates.len();
                CompletionOutcome::Consumed
            }
            Key::Up => {
                self.selected = self
                    .selected
                    .checked_sub(1)
                    .unwrap_or(self.candidates.len() - 1);
                CompletionOutcome::Consumed
            }
            Key::Tab if backwards => {
                self.selected = self
                    .selected
                    .checked_sub(1)
                    .unwrap_or(self.candidates.len() - 1);
                CompletionOutcome::Consumed
            }
            Key::Tab => {
                self.selected = (self.selected + 1) % self.candidates.len();
                CompletionOutcome::Consumed
            }
            Key::Enter => match self.current() {
                Some(candidate) => CompletionOutcome::Accepted(candidate.to_owned()),
                None => CompletionOutcome::Ignored,
            },
            _ => CompletionOutcome::Ignored,
        }
    }
}

/// 菜单对一次输入的处理结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuOutcome {
    /// 输入未被菜单识别。
    Ignored,

    /// 菜单消费了输入但没有产生选择。
    Consumed,

    /// 用户接受了一个条目。
    Accepted {
        /// 被接受条目在过滤结果中的下标。
        index: usize,

        /// 条目的稳定值。
        value: String,
    },

    /// 用户取消了菜单。
    Cancelled,
}

/// 带模糊过滤与选择状态的菜单。
///
/// `items` 是调用方提供的完整条目集合，`selected` 始终指向过滤结果中的当前下标；
/// 过滤结果为空时 `selected` 为 0 且不会确认。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Menu {
    items: Vec<MenuItem>,
    query: String,
    selected: usize,
}

impl Menu {
    /// 使用条目创建菜单。
    #[must_use]
    pub fn new(items: Vec<MenuItem>) -> Self {
        Self {
            items,
            query: String::new(),
            selected: 0,
        }
    }

    /// 返回全部条目。
    #[must_use]
    pub fn items(&self) -> &[MenuItem] {
        &self.items
    }

    /// 返回当前过滤输入。
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// 返回过滤结果中当前选中的下标。
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// 返回按过滤输入筛选后的条目下标，顺序与显示顺序一致。
    #[must_use]
    pub fn visible(&self) -> Vec<usize> {
        if self.query.is_empty() {
            return (0..self.items.len()).collect();
        }
        let mut scored = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| score(&self.query, &item.label).map(|score| (score, index)))
            .collect::<Vec<_>>();
        scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        scored.into_iter().map(|(_, index)| index).collect()
    }

    /// 返回当前选中条目。
    #[must_use]
    pub fn current(&self) -> Option<&MenuItem> {
        self.visible()
            .get(self.selected)
            .and_then(|index| self.items.get(*index))
    }

    /// 处理一次输入。
    pub fn handle_input(&mut self, event: InputEvent) -> MenuOutcome {
        match event.key {
            Key::Escape => MenuOutcome::Cancelled,
            Key::Enter => match self.current() {
                Some(item) if item.enabled => MenuOutcome::Accepted {
                    index: self.selected,
                    value: item.id.clone(),
                },
                _ => MenuOutcome::Consumed,
            },
            Key::Up => {
                self.move_selection(-1);
                MenuOutcome::Consumed
            }
            Key::Down => {
                self.move_selection(1);
                MenuOutcome::Consumed
            }
            Key::Backspace => {
                if self.query.pop().is_some() {
                    self.selected = 0;
                }
                MenuOutcome::Consumed
            }
            Key::Char(character)
                if !event.modifiers.control && !event.modifiers.alt && !character.is_control() =>
            {
                self.query.push(character);
                self.selected = 0;
                MenuOutcome::Consumed
            }
            _ => MenuOutcome::Ignored,
        }
    }

    /// 把选择移动到下一个启用条目。
    fn move_selection(&mut self, delta: isize) {
        let visible = self.visible();
        if visible.is_empty() {
            self.selected = 0;
            return;
        }
        let len = visible.len();
        let mut candidate = self.selected.min(len - 1);
        for _ in 0..len {
            candidate = if delta < 0 {
                (candidate + len - 1) % len
            } else {
                (candidate + 1) % len
            };
            if self.items[visible[candidate]].enabled {
                self.selected = candidate;
                return;
            }
        }
    }
}

/// 返回不区分大小写的子序列匹配分数；无匹配返回 `None`。
///
/// 连续命中与词首命中权重更高，靠前匹配优先，较长候选轻微扣分。
#[must_use]
pub fn score(query: &str, candidate: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let query = query.to_lowercase();
    let candidate_lower = candidate.to_lowercase();
    if let Some(position) = candidate_lower.find(&query) {
        let boundary = position == 0
            || candidate[..position]
                .chars()
                .next_back()
                .is_some_and(|character| !character.is_alphanumeric());
        return Some(100 + i64::from(boundary) * 25 - position as i64 - candidate.len() as i64 / 8);
    }
    let mut positions = Vec::new();
    let mut cursor = 0;
    for needle in query.chars() {
        let relative = candidate_lower[cursor..].find(needle)?;
        let position = cursor + relative;
        positions.push(position);
        cursor = position + needle.len_utf8();
    }
    let consecutive = positions
        .windows(2)
        .filter(|pair| pair[1] == pair[0] + 1)
        .count();
    let boundary = positions
        .iter()
        .filter(|position| {
            **position == 0
                || candidate[..**position]
                    .chars()
                    .next_back()
                    .is_some_and(|character| !character.is_alphanumeric())
        })
        .count();
    Some(
        consecutive as i64 * 20 + boundary as i64 * 15
            - positions[0] as i64
            - candidate.len() as i64 / 8,
    )
}

#[cfg(test)]
mod tests {
    use teal_workbench_core::{InputEvent, Key, KeyModifiers};

    use super::{Menu, MenuOutcome, score};
    use crate::workbench::ui::MenuItem;

    fn key(key: Key) -> InputEvent {
        InputEvent {
            key,
            modifiers: KeyModifiers::default(),
        }
    }

    fn menu() -> Menu {
        Menu::new(vec![
            MenuItem::enabled("open", "Open file"),
            MenuItem::disabled("hidden", "Disabled entry"),
            MenuItem::enabled("close", "Close buffer"),
        ])
    }

    #[test]
    fn filters_by_subsequence_and_keeps_the_selection_in_range() {
        let mut menu = menu();
        assert_eq!(menu.visible(), vec![0, 1, 2]);
        // 子序列匹配把 "cf" 落在 "Close buffer" 上。
        for character in "cf".chars() {
            assert_eq!(
                menu.handle_input(key(Key::Char(character))),
                MenuOutcome::Consumed
            );
        }
        assert_eq!(menu.query(), "cf");
        assert_eq!(menu.visible(), vec![2]);
        assert_eq!(menu.selected(), 0);
        assert_eq!(menu.current().map(|item| item.id.as_str()), Some("close"));

        // 回退过滤后选择回到开头。
        assert_eq!(
            menu.handle_input(key(Key::Backspace)),
            MenuOutcome::Consumed
        );
        assert_eq!(menu.query(), "c");
        assert_eq!(menu.selected(), 0);
        // 再回退一次恢复完整列表。
        assert_eq!(
            menu.handle_input(key(Key::Backspace)),
            MenuOutcome::Consumed
        );
        assert_eq!(menu.query(), "");
        assert_eq!(menu.visible().len(), 3);
    }

    #[test]
    fn movement_skips_disabled_entries_and_enter_accepts() {
        let mut menu = menu();
        assert_eq!(menu.handle_input(key(Key::Down)), MenuOutcome::Consumed);
        // 中间条目被禁用，必须跳到下一个启用条目。
        assert_eq!(menu.selected(), 2);
        assert_eq!(
            menu.handle_input(key(Key::Enter)),
            MenuOutcome::Accepted {
                index: 2,
                value: "close".to_owned()
            }
        );
        assert_eq!(menu.handle_input(key(Key::Up)), MenuOutcome::Consumed);
        assert_eq!(menu.selected(), 0);
        assert_eq!(menu.handle_input(key(Key::Escape)), MenuOutcome::Cancelled);
    }

    #[test]
    fn empty_filter_matches_every_candidate_and_unknown_input_bubbles() {
        assert_eq!(score("", "anything"), Some(0));
        assert!(score("zzz", "Open file").is_none());
        let mut menu = menu();
        assert_eq!(
            menu.handle_input(key(Key::Function(1))),
            MenuOutcome::Ignored
        );
        // 没有匹配项时回车不产生选择。
        for character in "zzz".chars() {
            menu.handle_input(key(Key::Char(character)));
        }
        assert!(menu.visible().is_empty());
        assert_eq!(menu.handle_input(key(Key::Enter)), MenuOutcome::Consumed);
    }
}
