# Spec Delta

## Purpose

Input capture turns host OS pointer and keyboard activity into a stream of capture events for delivery to remote peers, while guaranteeing that local input is never starved, trapped, or left in a held state when a capture session ends or a peer becomes unreachable.

## ADDED Requirements

### Requirement: Stream Progress Without Registered Handles
The capture event stream MUST keep producing events from the backend even when an event refers to a screen position with no associated capture handle. A dequeued event whose position has no handle MUST be discarded and the stream MUST continue polling the backend rather than suspending indefinitely.

#### Scenario: Event arrives for a position whose capture was just removed
- **WHEN** a capture handle is destroyed while the backend still has in-flight events for that position
- **THEN** those events are dropped and subsequent events for any position continue to be delivered to the consumer

#### Scenario: Session ends with the app still running
- **WHEN** the user disables or disconnects a client while backend events are queued for its position
- **THEN** the capture task continues processing requests instead of parking, and re-enabling the client resumes capture without restarting the application

### Requirement: Non-Blocking OS Input Callback
The OS input interception callback MUST never block waiting for another task. It MUST NOT hold a lock across event dispatch, and it MUST NOT wait for capacity in a bounded channel. Temporary backpressure MUST degrade to coalesced or dropped motion samples, never to a stalled callback.

#### Scenario: Runtime is busy with an unrelated long operation
- **WHEN** the single-threaded runtime is occupied and the OS delivers pointer or keyboard events
- **THEN** the interception callback returns promptly and the operating system does not disable the interception mechanism

#### Scenario: Event channel is full
- **WHEN** the callback cannot enqueue an event because the channel is at capacity
- **THEN** the callback still returns promptly, motion is coalesced or dropped, and a later event reflects the accumulated movement

### Requirement: Conditional Release On Send Failure
Capture MUST distinguish a not-yet-established connection from a failed connection. A send that fails because a connection is still being established MUST NOT end the capture session; only a peer that does not acknowledge entry within the configured timeout MUST release it.

#### Scenario: First barrier crossing on a cold connection
- **WHEN** the cursor crosses into a remote client whose connection has not been established yet
- **THEN** the session stays active, the connection completes in the background, and input begins flowing without requiring a second crossing

#### Scenario: Peer never acknowledges
- **WHEN** the peer accepts no connection and sends no acknowledgement before the entry timeout elapses
- **THEN** the session releases and local pointer control returns

### Requirement: Release Notifies The Peer Before Teardown
When a capture session ends for any reason, the system MUST emit the outstanding key-release and modifier-clear events for every key the capture forwarded as held, then notify the peer that capture was released. This MUST hold for release-bind chords, remote leave requests, peer send failures, client removal, application quit, and OS-initiated shutdown.

#### Scenario: Application quits while capturing
- **WHEN** the user quits the app, the process receives a termination signal, or the OS begins a shutdown while a remote client is captured
- **THEN** the peer receives key-up events for held keys, a cleared modifier state, and a leave event before the connection closes

#### Scenario: Connection already broken at release time
- **WHEN** the session ends because the connection failed
- **THEN** no new connection attempt is spawned solely to deliver the release notice, and the peer recovers through its own liveness timeout

### Requirement: Cursor State Restored On Abnormal Termination
The host MUST restore the local cursor and release any capture state it created when interception is disabled by the operating system, when the peer disconnects, or when the session ends unexpectedly.

#### Scenario: Interception disabled by the OS mid-session
- **WHEN** the OS disables the interception mechanism while a remote client is captured
- **THEN** the host cursor is made visible again and the capture state is cleared

#### Scenario: Backend reports capture termination
- **WHEN** the capture backend reports that it stopped supplying events
- **THEN** the host is left with a visible, controllable cursor and no lingering capture state
