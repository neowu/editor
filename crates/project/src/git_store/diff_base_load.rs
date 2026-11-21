use std::future::Future;
use std::mem;

use anyhow::Context as _;
use anyhow::Result;
use futures::channel::oneshot;
use git::repository::RepoPath;
use util::ResultExt as _;

use super::buffer_git_state::DiffBasesChange;
use super::decode_git_text;

/// One buffer's diff base load. Loads queued while their job waits share one
/// `git cat-file` process.
pub(super) struct DiffBaseLoad {
    pub(super) repo_path: RepoPath,
    pub(super) kind: DiffBaseLoadKind,
}

pub(super) enum DiffBaseLoadKind {
    /// HEAD and index text.
    Committed(oneshot::Sender<Result<DiffBasesChange>>),
    /// Index text.
    Staged(oneshot::Sender<Result<Option<String>>>),
}

impl DiffBaseLoad {
    pub(super) fn revisions(&self) -> Vec<String> {
        let path = self.repo_path.as_unix_str();
        match self.kind {
            DiffBaseLoadKind::Committed(_) => vec![format!("HEAD:{path}"), format!(":{path}")],
            DiffBaseLoadKind::Staged(_) => vec![format!(":{path}")],
        }
    }

    /// Sends this load's result, given the result of loading its `revisions`.
    pub(super) fn finish(self, texts: Result<Vec<Option<Vec<u8>>>>) {
        match self.kind {
            DiffBaseLoadKind::Committed(result_tx) => {
                let result = texts.and_then(|texts| {
                    let mut texts = texts.into_iter();
                    let head = texts.next().flatten().map(decode_git_text).transpose()?;
                    let index = texts.next().flatten().map(decode_git_text).transpose()?;
                    Ok(if head == index { DiffBasesChange::Both(head) } else { DiffBasesChange::Each { index, head } })
                });
                result_tx.send(result).ok();
            }
            // Like `GitRepository::load_index_text`, treats an unreadable index entry as a
            // missing one, so the unstaged diff still opens.
            DiffBaseLoadKind::Staged(result_tx) => {
                let index = texts
                    .context("failed to load index text")
                    .log_err()
                    .and_then(|texts| texts.into_iter().next().flatten());
                result_tx.send(index.map(decode_git_text).transpose()).ok();
            }
        }
    }
}

/// Loads all groups' revisions with one `load` call (one `git cat-file` process). One unreadable
/// revision fails the whole batch, so after a failure each group is retried on its own.
pub(super) async fn load_revision_groups<F, Fut>(
    load: F,
    mut groups: Vec<Vec<String>>,
) -> Vec<Result<Vec<Option<Vec<u8>>>>>
where
    F: Fn(Vec<String>) -> Fut,
    Fut: Future<Output = Result<Vec<Option<Vec<u8>>>>>,
{
    // `git cat-file --batch` reads one revision per line, so a newline in a revision fails its
    // batch. Loading those groups on their own keeps them from sending every other group to the
    // retries below.
    let separate_groups = groups
        .iter_mut()
        .enumerate()
        .filter(|(_, group)| group.iter().any(|revision| revision.contains('\n')))
        .map(|(index, group)| (index, mem::take(group)))
        .collect::<Vec<_>>();
    let mut results = match load(groups.concat()).await {
        Ok(texts) => {
            let mut texts = texts.into_iter();
            groups.iter().map(|group| Ok(texts.by_ref().take(group.len()).collect())).collect()
        }
        Err(error) if groups.len() == 1 => vec![Err(error)],
        Err(error) => {
            log::warn!("retrying {} revision groups separately: {error:#}", groups.len());
            let mut results = Vec::with_capacity(groups.len());
            for group in groups {
                results.push(load(group).await);
            }
            results
        }
    };
    for (index, group) in separate_groups {
        if let Some(result) = results.get_mut(index) {
            *result = load(group).await;
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;
    use futures::executor::block_on;

    use super::*;

    fn revisions(revisions: &[&str]) -> Vec<String> {
        revisions.iter().map(|revision| (*revision).to_owned()).collect()
    }

    /// Echoes each revision back as its text; fails any batch containing a "bad" or multi-line revision.
    async fn echo(revisions: Vec<String>) -> Result<Vec<Option<Vec<u8>>>> {
        if revisions.iter().any(|revision| revision.contains("bad") || revision.contains('\n')) {
            return Err(anyhow!("unreadable"));
        }
        Ok(revisions
            .into_iter()
            .map(|revision| (!revision.contains("missing")).then(|| revision.into_bytes()))
            .collect())
    }

    fn texts(result: &Result<Vec<Option<Vec<u8>>>>) -> Option<Vec<Option<&str>>> {
        let texts = result.as_ref().ok()?;
        Some(texts.iter().map(|text| text.as_deref().map(|text| str::from_utf8(text).expect("utf-8"))).collect())
    }

    #[test]
    fn load_revision_groups_splits_one_batch_by_group() {
        let calls = std::cell::Cell::new(0);
        let load = |revisions| {
            calls.set(calls.get() + 1);
            echo(revisions)
        };
        let groups = vec![revisions(&["HEAD:a", ":a"]), revisions(&[":missing"]), Vec::new(), revisions(&[":c"])];
        let results = block_on(load_revision_groups(load, groups));
        assert_eq!(calls.get(), 1);
        assert_eq!(
            results.iter().map(texts).collect::<Vec<_>>(),
            [Some(vec![Some("HEAD:a"), Some(":a")]), Some(vec![None]), Some(vec![]), Some(vec![Some(":c")])]
        );
    }

    #[test]
    fn load_revision_groups_retries_each_group_after_a_failed_batch() {
        let groups = vec![revisions(&["HEAD:a", ":a"]), revisions(&[":bad"]), revisions(&[":c"])];
        let results = block_on(load_revision_groups(echo, groups));
        assert_eq!(
            results.iter().map(texts).collect::<Vec<_>>(),
            [Some(vec![Some("HEAD:a"), Some(":a")]), None, Some(vec![Some(":c")])]
        );
    }

    #[test]
    fn load_revision_groups_loads_multiline_revisions_separately() {
        let calls = std::cell::Cell::new(0);
        let load = |revisions| {
            calls.set(calls.get() + 1);
            echo(revisions)
        };
        let groups = vec![revisions(&[":a"]), revisions(&[":b\nc"]), revisions(&[":d"])];
        let results = block_on(load_revision_groups(load, groups));
        // One batch for the readable groups, one attempt for the multi-line group.
        assert_eq!(calls.get(), 2);
        assert_eq!(
            results.iter().map(texts).collect::<Vec<_>>(),
            [Some(vec![Some(":a")]), None, Some(vec![Some(":d")])]
        );
    }

    #[test]
    fn committed_load_reports_head_and_index() {
        let load = |texts: Result<Vec<Option<Vec<u8>>>>| {
            let (result_tx, mut result_rx) = oneshot::channel();
            let load = DiffBaseLoad {
                repo_path: RepoPath::new("a.txt").expect("valid path"),
                kind: DiffBaseLoadKind::Committed(result_tx),
            };
            assert_eq!(load.revisions(), ["HEAD:a.txt", ":a.txt"]);
            load.finish(texts);
            result_rx.try_recv().expect("not canceled").expect("result sent")
        };
        assert!(matches!(
            load(Ok(vec![Some(b"same".to_vec()), Some(b"same".to_vec())])),
            Ok(DiffBasesChange::Both(Some(text))) if text == "same"
        ));
        assert!(matches!(
            load(Ok(vec![Some(b"head".to_vec()), None])),
            Ok(DiffBasesChange::Each { index: None, head: Some(head) }) if head == "head"
        ));
        assert!(matches!(load(Ok(vec![None, None])), Ok(DiffBasesChange::Both(None))));
        load(Err(anyhow!("unreadable"))).unwrap_err();
    }

    #[test]
    fn staged_load_treats_a_failed_load_as_missing() {
        let load = |texts: Result<Vec<Option<Vec<u8>>>>| {
            let (result_tx, mut result_rx) = oneshot::channel();
            let load = DiffBaseLoad {
                repo_path: RepoPath::new("a.txt").expect("valid path"),
                kind: DiffBaseLoadKind::Staged(result_tx),
            };
            assert_eq!(load.revisions(), [":a.txt"]);
            load.finish(texts);
            result_rx.try_recv().expect("not canceled").expect("result sent")
        };
        assert_eq!(load(Ok(vec![Some(b"index".to_vec())])).expect("loaded"), Some("index".to_owned()));
        assert_eq!(load(Ok(vec![None])).expect("loaded"), None);
        assert_eq!(load(Err(anyhow!("unreadable"))).expect("loaded"), None);
    }
}
