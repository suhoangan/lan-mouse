# Spec Delta

## Purpose

Connection lifecycle covers how two Lan Mouse instances establish, authenticate, monitor, and tear down DTLS sessions so that either machine can drive the other, one approval holds for both directions, and a session that ends is observed as ended by both sides.

## ADDED Requirements

### Requirement: Handshake Not Cancelled Mid-Flight
An inbound handshake that has begun MUST be allowed to complete or time out on its own schedule. The listener MUST NOT abort an in-progress handshake as a side effect of a periodic loop iteration.

#### Scenario: Peer handshake is slow
- **WHEN** a peer's DTLS handshake takes longer than the listener's periodic tick because the peer is waking from sleep or retrying over a congested link
- **THEN** the handshake completes successfully and the connection is accepted

#### Scenario: Listener must remain responsive
- **WHEN** no handshake is in progress
- **THEN** the listener continues to accept new connections without the periodic-interval workaround interfering

### Requirement: Bidirectional Authorization From A Single Approval
Authorizing a peer on one machine MUST authorize traffic in both directions between those two machines. An approval MUST identify the peer by certificate fingerprint, and that fingerprint MUST also be accepted as the identity of connections initiated toward that peer.

#### Scenario: Peer approved on the receiving machine
- **WHEN** the user approves a peer's fingerprint on machine A, and machine A then initiates a connection to that peer
- **THEN** the fingerprint observed on the outgoing connection is recorded so the peer's inbound connections to machine A are accepted without a second approval

#### Scenario: Peer-initiated traffic from an unapproved machine
- **WHEN** a machine whose fingerprint has never been approved attempts to connect inbound
- **THEN** the connection is rejected and the user is prompted to review that fingerprint

### Requirement: Peer Notified Of Session End
When a session ends because one side quit, shut down, or stopped the service, the remaining side MUST release any input state attributed to that peer without waiting for a liveness timeout.

#### Scenario: Remote side quits the application
- **WHEN** the peer quits the application or its machine begins a shutdown
- **THEN** the local side observes the disconnect and releases held keys and modifiers for that peer

#### Scenario: Abrupt peer loss
- **WHEN** the peer disappears without a graceful notice, such as power loss or a network drop
- **THEN** the local side detects the loss through its liveness checks and releases input state within the documented liveness window

### Requirement: Liveness Detection Survives Packet Loss
Liveness checks MUST tolerate isolated packet loss before concluding a peer is gone, and MUST conclude a peer is gone when repeated checks go unanswered.

#### Scenario: Isolated lost datagram
- **WHEN** a single liveness probe or its response is lost but the peer is reachable
- **THEN** the session remains established

#### Scenario: Peer stops responding
- **WHEN** consecutive liveness probes go unanswered over the liveness window
- **THEN** the session is closed and the peer's input state is released

### Requirement: Clipboard Transfer Does Not Block Input
Clipboard reads and writes MUST NOT execute inside the input delivery path. Input event latency MUST NOT depend on the availability or responsiveness of the system clipboard.

#### Scenario: Clipboard is slow or unavailable
- **WHEN** the system clipboard is locked, slow, or unsupported while pointer and keyboard events are flowing
- **THEN** input events continue to be delivered with unchanged latency

#### Scenario: Clipboard content received while capturing
- **WHEN** clipboard content arrives from the peer during an active capture session
- **THEN** the content is applied without stalling input delivery

### Requirement: Graphical Frontend Starts Without A Console Window
On Windows, launching the packaged application outside a terminal MUST NOT open a console window. Launching it from a terminal MAY share that terminal for log output.

#### Scenario: Launched from Explorer or the Start menu
- **WHEN** the packaged application is started without an attached console
- **THEN** no console window appears

#### Scenario: Launched from a terminal
- **WHEN** the packaged application is started from a terminal
- **THEN** log output is still visible in that terminal
