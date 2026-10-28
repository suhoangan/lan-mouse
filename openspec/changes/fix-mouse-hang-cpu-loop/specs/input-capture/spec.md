# Spec Delta

## Purpose

Input capture coordinates the detection of pointer edge crossings, captures physical mouse and keyboard devices on the host operating system, and streams low-level events to the remote machine while safeguarding local input against deadlock and starvation.

## ADDED Requirements

### Requirement: Continuous Edge Control Pass-Through
When input capture is active for a remote device, mouse movements in all directions on the physical mouse surface MUST be captured and streamed to the active remote client as relative motion events without being prematurely cancelled by local coordinate fluctuations.

#### Scenario: User moves mouse freely on remote desktop
- **WHEN** the mouse enters a remote client on the right screen edge and moves leftwards on the remote screen
- **THEN** relative motion deltas with negative X values are streamed to the remote client, and the local capture session remains active without oscillating.

### Requirement: Unresponsive Peer Connection Timeout
When input capture attempts to enter a remote client, the system MUST NOT indefinitely suppress local mouse input if the peer does not acknowledge the entry request.

#### Scenario: Remote client is offline or unreachable
- **WHEN** the local cursor crosses into a remote client boundary but no connection acknowledgment (`Ack`) is received within 1.5 seconds
- **THEN** the local capture session automatically releases and local pointer control is fully restored to the host OS.

### Requirement: Manual Emergency Capture Release
The host system MUST provide a reliable hardware shortcut chord that immediately breaks any active or pending input capture session regardless of network state.

#### Scenario: User triggers emergency release bind
- **WHEN** the user presses the configured release bind chord (default `Ctrl + Shift + Alt + Meta`)
- **THEN** the active capture session terminates immediately, all held modifiers are cleared, and control returns to the local desktop.
