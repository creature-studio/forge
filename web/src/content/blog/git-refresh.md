A terminal Git workspace has two jobs: respond to input and stay consistent with a working tree that other processes can change. A file watcher helps, but processing every notification is not the same as keeping the displayed state correct.

Engineering note based on Forge revision [1ba47ca](https://github.com/creature-studio/forge/commit/1ba47ca). Prepared with AI assistance. This is a source-based design explanation, not an application performance benchmark.

## Notifications are invalidation hints, not a repository history

Editors can save a file through temporary files and renames. Build scripts or coding assistants can change many paths in a burst. Treating every filesystem event as a durable, ordered record is a poor basis for a Git status view: the authoritative state is the repository itself.

Forge's `WatchMailbox` uses a bounded Tokio MPSC channel. The watcher callback uses `try_send` instead of waiting for the consumer. When the queue fills, it marks an atomic overflow flag. The consumer can clear the flag with an atomic swap.

```rust
pub(super) fn offer(&self, event: notify::Event) {
    if let Err(mpsc::error::TrySendError::Full(_)) =
        self.sender.try_send(event)
    {
        self.mark_overflow();
    }
}
```

The mailbox intentionally does not preserve every event under overload. Its contract is to signal that a snapshot may be stale. A consumer must respond to overflow by refreshing authoritative state; the flag does not reconstruct the discarded event. This bounds the number of queued events, not the memory use of the entire application.

Source: [watcher mailbox and filtering implementation](https://github.com/creature-studio/forge/blob/1ba47ca/crates/forge-app/src/auto_refresh.rs).

## Git worktree metadata is not always a directory

A linked Git worktree may have a `.git` pointer file instead of a `.git/` directory. Forge resolves the gitdir pointer and the optional commondir pointer to locate private and shared metadata. That matters because HEAD and the index can be worktree-specific, while refs can live in the shared repository.

The filter considers relevant metadata and working-tree changes, while excluding access-only events, transient index locks, and object-store or log churn under the project's .git directory. The resulting notification is still only a reason to refresh, not a new Git status value.

## Separate the Git read and write paths

The `GitService` facade uses a gix-backed implementation for status and graph reads and a Git CLI backend for operations including writes and blame. Its synchronous read methods are explicitly documented as blocking; they are intended to be called from a blocking worker rather than from a render loop.

This separation lets UI scheduling and Git operations evolve independently. It does not by itself establish a speed or memory advantage over another Git client.

Source: [GitService facade](https://github.com/creature-studio/forge/blob/1ba47ca/crates/forge-git/src/lib.rs).

## Reproduce the mailbox and worktree tests

Check out the cited revision in a separate clone and run the focused test module:

```sh
git checkout 1ba47ca
cargo test -p forge-app auto_refresh::tests --locked
```

Three focused tests cover overflow recovery, linked-worktree path resolution and filtering Git object churn. The prior Linux development-node run passed these tests. No application refresh latency, CPU savings, Windows or macOS test result is claimed here.

For a wider real-PTY smoke test, consult [smoke-auto-refresh.py](https://github.com/creature-studio/forge/blob/1ba47ca/scripts/smoke-auto-refresh.py) and its prerequisites. That script was not rerun for this note.

## Takeaway

When a UI observes changing state, bound the notification queue, make overflow explicit and refresh authoritative state. The same principle applies whether a developer is typing in another editor or reviewing the output of a coding assistant.

Explore [Forge's Git review workflow](/en/features/git/), read the [user guide](/en/guide/), or inspect the [open-source project](https://github.com/creature-studio/forge).
