# Non-blocking TUI Submission Design

## Problem

Submitting any message, including a greeting such as `你好`, awaits the service call inside the terminal event handler. The event loop cannot draw the cleared composer, the user message, progress, or an error until that call returns. A slow or timed-out model therefore makes the TUI appear frozen and prevents normal keyboard handling.

## Required behavior

- Enter accepts a non-empty message and returns control to the TUI event loop immediately.
- The next draw shows the submitted user message and an in-progress status without waiting for the model.
- Model and service work runs outside the terminal event loop.
- Success, validation failure, provider timeout, and other service failures return through an internal completion event and are rendered in the transcript.
- While one submission is running, another Enter does not create a duplicate request; the composer remains usable after completion.
- Ctrl-C and `/quit` remain responsive while a request is in flight, and shutdown aborts the background request safely.
- Existing Query Run event handling, clarification behavior, slash commands, and durable run recovery remain unchanged.

## Design

Split submission into a fast foreground transition and a background operation. The foreground parses the composer, records the user-visible pending state, and creates a future for the controller operation. The event loop owns that future and polls it alongside terminal events, service events, signals, and redraw ticks. When it resolves, the event loop applies the result to the existing transcript and clears the pending state.

Use one in-flight foreground command because `TuiController` owns mutable focus, subscription, and idempotency state. This avoids sharing or duplicating the controller. Non-message commands may keep their current synchronous path when they do not call the model; message and clarification submission use the in-flight path.

## Error handling

Service errors are converted with the existing safe user-facing formatter and appended as transcript errors. The completion path always releases the in-flight guard. Dropping the TUI drops the pending future, so no detached task can mutate controller state after terminal restoration.

## Verification

Add a deterministic delayed service test at the event-loop/controller seam. It must prove that submitting `你好` produces a renderable user message and progress state before the delayed reply is released, terminal input remains processable, and the eventual reply or error renders after completion. Retain the existing focused TUI and runtime suites, then exercise the compiled binary through a pseudo-terminal.
