# asterinas-tpm-verus

Verus-based verification work for the Asterinas TPM module stack.

## Overview

This repository focuses on making TPM driver behavior explicit, auditable, and mechanically checked.
The core crate (`tpm-core`) models TPM command transport, session authorization, context management,
and boot-time sequencing with Verus-oriented specifications and proofs.

At a high level, this codebase aims to guarantee:

- Memory-safe command/response handling with strict length discipline.
- Protocol-safe TPM state transitions for locality, FIFO, and command lifecycle.
- Clear separation between recoverable failures (timeout, retryable TPM return codes) and hard failures.
- Session-authenticated command flow where response bytes are only exposed after authorization checks.

## Repository Layout

- `tpm-core/src/`
	- Main verification-oriented TPM logic.
	- Includes transport (`tis_core`, `xfer`), CRB path (`crb_core`), boot (`boot`),
		session/auth (`session`, `auth`, `secure`), and space/context management (`module`, `table`).
- `src/`
	- Mirror/integration-facing tree in this workspace.

The `tpm-core` directory should be treated as the source of truth for the core contracts.

## What Verus Verification Covers

The implementation is structured so that key safety invariants are expressed in executable logic plus
specification-friendly structure. In practical terms, the verified intent includes the following categories.

### 1. Buffer and Length Safety

- Command validity checks ensure that the header length field matches the actual transmitted length.
- Response paths validate minimum header size before parsing fixed offsets.
- Sub-slice operations are guarded by explicit offset/length relationships.
- CRB and TIS data movement is bounded by declared capacities and checked arithmetic.

Why this matters:

- TPM protocol parsing bugs are often length bugs first; proving and enforcing these conditions removes
	an entire class of out-of-bounds and desynchronization failures.

### 2. Locality and Command Lifecycle Discipline

- Transport operations are designed around a request-use / transmit / relinquish lifecycle.
- The code tracks whether locality is currently held and guarantees release on all normal error exits.
- Reset/cleanup paths are explicit when partial writes may have happened.

Why this matters:

- A locality leak is both a liveness and safety problem: future commands can stall or race with stale state.

### 3. Polling-Termination and Timeout Semantics

- Waiting is modeled in poll budgets (iteration counts), not wall-clock proof assumptions.
- Delay is abstracted behind a platform primitive, while termination relies on decreasing counters.

Why this matters:

- Proof obligations stay stable across hardware with different timing characteristics.

### 4. Retry and Error Classification Discipline

- Retry policy is centralized in transport (`xfer`) and tied to explicit TPM return-code classes.
- Hard bus/protocol failures are not silently converted into soft retries.

Why this matters:

- Correct error taxonomy is necessary for safe recovery policy at upper layers.

### 5. Session Authorization Gating

- Authenticated command flow requires MAC validation before response parameter exposure.
- Response access is mediated by an authorization witness type (`Authenticated`) instead of raw bytes.
- Session state progression (idle/pending/closed) is explicit and non-accidental.

Why this matters:

- This prevents a common integration bug: consuming untrusted response bytes as if they were authenticated.

## Verification Boundary and Trusted Computing Base (TCB)

Verus reasoning is only as strong as the assumptions at the boundary. The following components are in the
trusted base and must satisfy their contracts for the verification claims to transfer to runtime behavior.

### Hardware/Platform TCB

- `TisPhy` / `TisMmioBackend` implementations.
- `CrbPhy` implementations.
- MMIO mapping correctness and register access ordering guarantees.
- Concurrency control around a TPM device instance.

### Cryptographic TCB

- `Sha256Ctx`, `HmacSha256Ctx`, `AesCfb`, `NonceSource` implementations.
- Entropy quality and nonce non-reuse guarantees from the platform RNG source.

If these implementations violate their contracts, proofs about upper-layer logic do not imply runtime safety.

## Kernel/Platform Contract Required by `tpm-core`

This section is the integration checklist. Treat each item as required unless marked advisory.

### A. MMIO and Register Access Contract (Required)

1. Correct mapping and lifetime
- TPM register windows and command/response buffers must remain valid and accessible for the full
	command round-trip.

2. Width and endianness correctness
- 8-bit and 32-bit operations must match device semantics.
- TIS 32-bit accesses are interpreted in little-endian form by the transport layer; platform code must not
	apply conflicting byte-order transforms.

3. Error reporting fidelity
- Hardware access failures must return explicit errors.
- Do not return fabricated values on failure paths.

4. Access ordering
- Do not reorder reads/writes in a way that changes TPM-visible protocol behavior.
- For CRB, `fence()` must ensure command-buffer writes are visible before writing START.

### B. FIFO and Buffer Movement Contract (Required)

1. TIS FIFO read/write semantics
- `read_fifo(addr, out, off, n)` must only modify `out[off..off+n]`.
- `write_fifo(addr, data, off, n)` may fail after a prefix write; upper layers already model this.

2. FIFO reset semantics
- `reset_fifo` must attempt to put TPM into command-ready state to clear partial commands.

3. CRB bounded access
- `write_cmd` and `read_rsp` must honor provided bounds and offsets exactly.
- Response writes must never exceed destination slice capacity.

### C. Delay, Polling, and Timeout Contract (Required)

1. Delay progress
- `delay()` need not be millisecond-accurate, but it must make forward progress and return.

2. Timeout/error separation
- Keep timeout/protocol/bus failures distinguishable.
- Do not collapse these classes into a single opaque error at the platform edge.

3. Retry ownership
- Transport retries are already handled in `xfer`; platform layers should not add hidden retry loops for
	the same command flow.

### D. Locality and Concurrency Contract (Required)

1. Single active command stream per TPM instance
- Serialize access to a given TPM backend object (for example, one lock per TPM device).

2. No external locality interference
- While a command is in flight, do not allow out-of-band accesses that can mutate the same locality state.

3. Reentrancy discipline
- Do not call into the same transport object recursively from callbacks/interrupt paths.

### E. Cryptography and Nonce Contract (Required for Authorized Sessions)

1. Algorithm conformance
- SHA-256 and HMAC-SHA256 implementations must be standard-conformant.
- AES-CFB must use the key/IV layout expected by `tpm-core`.

2. Nonce properties
- Nonces must not repeat across session rounds.
- Nonces should be unpredictable (cryptographic RNG quality or equivalent).

3. Secret handling hygiene
- Avoid logging key material, passphrases, MAC keys, or plaintext authorization payloads.
- Clear temporary secret buffers when feasible.

### F. Context I/O Contract (Required if Context Save/Load is used)

1. Offset and consumed-length consistency
- `load/save` must report consumed/written lengths consistent with actual bytes.

2. Idempotent flush
- `flush(handle)` should be safe to call on stale/already-freed handles.

3. Not-found fidelity
- Report missing handles as `NotFound` rather than fabricating success.

## Verus-Specific Notes for Reviewers

When reviewing or extending the verified code, these are the practical interpretation rules.

1. Proven properties are mostly control-flow and shape properties.
- Examples: bounded loops, state transitions, checked offsets, consistent header/length relations.

2. Platform-side semantic assumptions are explicit and intentionally not re-proved inside `tpm-core`.
- Examples: MMIO ordering, true hardware errors, lock discipline, real entropy quality.

3. "Verified" does not mean "hardware cannot misbehave."
- It means the software side enforces the declared invariants under declared assumptions.

4. Contract drift is the main long-term risk.
- If backend semantics change but trait-level assumptions are not updated, proofs can remain green while
	deployment safety regresses.

## Interpreting Verus Claims in Deployment

A practical way to read the verification story is:

- The core logic proves: "if platform and crypto contracts hold, driver invariants hold."
- The platform must prove operationally: "our backend honors those contracts under scheduler, MMIO,
	interrupt, and memory-order realities."

Only when both statements are true do you get end-to-end safety confidence.

## Recommended Integration Validation

Before production use, run fault-injection and stress checks that correspond directly to the contracts above:

1. Concurrency stress
- Confirm only one active command stream exists per TPM instance under heavy parallel callers.

2. MMIO fault injection
- Inject read/write failures and verify correct propagation as bus/phy errors.

3. Partial FIFO write fault
- Force mid-transfer failure and verify next command is not contaminated by stale bytes.

4. CRB fence validation
- Demonstrate failures when fence is intentionally removed, and recovery when restored.

5. Session authorization checks
- Validate that response parameters are inaccessible before successful MAC verification.

## Current Scope and Future Work

Current scope in this repository emphasizes TPM transport and session safety properties.
Future extensions can include:

- More explicit machine-checked contracts for backend memory-order assumptions.
- Extended verification of integration glue around interrupts and cancellation paths.
- Additional end-to-end proofs that connect kernel lock discipline to non-interference assumptions.

## Summary

`tpm-core` provides a verification-oriented TPM foundation, but deployment safety is joint work between
proved logic and trustworthy platform backends. Use the contract checklist in this README as a required
acceptance gate for kernel integration.
