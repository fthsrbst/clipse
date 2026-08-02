# Sync protocol

The wire contract between two `clipsed` instances. This is the specification
`clipse-net` and `clipse-sync` are built against; change it here first.

`clipse_core::PROTOCOL_VERSION` gates compatibility. Anything in this document
that changes shape requires bumping it.

## 1. Trust model

Two devices talk only if the user paired them by hand (six digits typed from one
screen to the other, see
`clipse-crypto`). There is no server, no directory, no account, and no relay.
A device that has not been paired cannot get past the handshake.

The threat we defend against is an attacker on the same LAN, or anyone who
obtains a device that was later removed. We do **not** defend against an
attacker with code execution on a currently-paired device — at that point they
have the clipboard anyway.

## 2. Addressing: one ordered candidate list

Pairing records every address a peer advertises. Dialling walks the list in
order and takes the first that connects:

1. **LAN addresses**, refreshed by mDNS (`_clipse._udp.local.`) whenever the
   peer is on the same network.
2. **The tailnet address**, resolved from `tailscale status --json`.

There is no multicast on a tailnet, so mDNS never discovers a remote peer; the
tailnet address is only ever known because it was recorded at pairing time and
re-resolved by name. If Tailscale is not installed the list simply has no
tailnet entry and Clipse is a LAN-only product — that is a supported
configuration, not a degraded one.

Both kinds of address are dialled over the same QUIC stack. `clipse-net`
exposes a `Transport` trait so a future transport (Bluetooth, a relay someone
runs themselves) can be added without touching `clipse-sync`.

### mDNS record

Service `_clipse._udp.local.`, TXT keys:

| Key | Value |
| --- | --- |
| `v` | `PROTOCOL_VERSION` |
| `id` | `DeviceId` (UUID) |
| `fp` | first 8 hex of the static-key fingerprint |
| `label` | device label, UTF-8 |
| `os` | `windows` \| `macos` \| `linux` |

The public key itself is not advertised. `fp` is enough for a paired peer to
recognise a known device and for an unpaired one to learn nothing useful.

## 3. Session establishment

1. QUIC connection to a candidate address. The QUIC TLS layer uses a
   self-signed per-device certificate and is **not** the authentication
   boundary — it is there for transport security and 0-RTT, nothing more.
2. On the first bidirectional stream, a **Noise_IK** handshake
   (`Noise_IK_25519_ChaChaPoly_BLAKE2s`). The initiator already knows the
   responder's static key from pairing, which is precisely what IK is for.
3. The responder rejects the handshake if the initiator's static key is not in
   its paired set, or if the epoch in `Hello` is older than its own.

Authentication lives in Noise, not in TLS, because the identity we care about
is the one the user confirmed during pairing — not whatever certificate a
socket happens to present. Yes, this encrypts twice; the cost is irrelevant
next to getting trust right.

That first stream stays open for the session and carries every control message.

## 4. Messages

MessagePack inside the Noise session, length-prefixed exactly as in
`clipse-ipc::codec`.

```
Hello      { device, epoch, protocol, max_hlc, label, platform }
Summary    { entries: [ClipSummary], complete: bool }
Want       { hashes: [ContentHash] }
Push       { clip: Clip }            // payloads inline when small
BlobOffer  { digest, size, chunk_size }
BlobWant   { digest, from_chunk }
BlobChunk  { digest, index, bytes }
BlobEnd    { digest }
Ack        { hlc }
Bye        { reason }
```

`ClipSummary` is `{ hash, hlc, kind, pinned, deleted, total_size }` — enough to
decide whether the peer needs it without shipping any content.

## 5. The exchange

**One side talks at a time, and the dialler goes first.** A symmetric exchange
where both ends write their whole summary before reading deadlocks the moment
both summaries exceed the QUIC flow-control window — each side is blocked
writing and neither is reading. Strict alternation costs one round trip and
removes the failure mode entirely:

1. Dialler sends `Hello`; responder replies `Hello`.
2. **Dialler's turn**: it sends `Summary` pages, the responder answers `Want`,
   the dialler sends the wanted `Push`es, the responder sends `Ack`.
3. **Responder's turn**: the same, roles swapped.

A Noise session has independent send and receive nonces, so a future version
could split the link and run both directions concurrently. That is not worth
doing until someone measures the round trip actually mattering.

On connect each side sends `Hello` with its `max_hlc`. Then:

1. Each side sends `Summary` for everything it has since the peer's `max_hlc`,
   in HLC order, paged with `complete: false` until the last page.
2. The receiver answers `Want` for the hashes it does not have, or has with an
   older HLC.
3. The sender replies `Push` per wanted clip. Payloads at or below
   `INLINE_MAX_BYTES` (64 KiB) ride inside the `Push`.
4. Anything larger becomes a `BlobOffer` per payload. The receiver answers
   `BlobWant { from_chunk }` and gets a stream of `BlobChunk`, then `BlobEnd`.
   Chunks are 256 KiB and travel on their own unidirectional QUIC stream so a
   large image cannot stall control traffic.
5. `Ack` carries the highest HLC durably stored, so a reconnect resumes rather
   than restarting.

`from_chunk` is what makes a transfer resumable: a laptop that sleeps mid-image
continues where it stopped instead of re-sending 40 MB.

## 6. Merge rules

Clip identity is `ContentHash`. Two devices copying the same bytes produce the
same clip.

- **Unknown hash** → insert.
- **Known hash** → merge metadata with **last-writer-wins on the HLC**:
  `pinned` and `deleted` take the value from the higher HLC. The HLC's device
  id breaks ties, so both sides independently reach the same answer.
- Content is never merged, only metadata — content is immutable by
  construction, because changing it changes the hash.
- **Deletions replicate.** A delete is a tombstone with its own HLC, not a
  removed row. Tombstones are purged only after every paired device has
  acknowledged an HLC past them.

## 7. Loop guard

A clip arriving from a peer is written to the local clipboard, which the local
watcher then observes. Without a guard that capture would be re-broadcast and
bounce forever. Three independent defences, because one is not enough:

1. **Platform layer.** `clipse-clipboard`'s `write()` records the hash it wrote;
   the watcher drops the next matching capture as `SuppressionReason::OwnWrite`.
2. **Sync layer.** A clip is never sent back to the device its HLC names as
   origin, and a hash received within the last 30 seconds is not re-broadcast.
3. **Store layer.** Insert is content-addressed, so a bounced clip is a dedup
   no-op rather than a new row.

Layer 1 is the one that must not be skipped; 2 and 3 exist because platform
clipboards are unreliable narrators and a dropped notification would otherwise
turn into an infinite loop.

## 8. Privacy invariants

These hold at every layer and are the reason the capture path is separate from
the sync path:

- A clip suppressed as sensitive never reaches the store, so it can never reach
  the network. There is no code path from capture to socket that bypasses the
  store.
- The IPC `Suppressed` event carries a reason, never content.
- No message in this protocol is sent to anything but a paired device.

## 9. Failure behaviour

- A peer that fails to handshake is retried with exponential backoff, capped;
  a peer that fails **authentication** is not retried and is surfaced in the UI,
  because that means either a removed device or something worth looking at.
- A `BlobChunk` stream that dies mid-transfer leaves the clip in the history as
  incomplete (`Clip::is_complete` is false) with the payload marked
  `PayloadBody::Blob`; it resumes on the next session.
- A blob whose assembled bytes do not match its digest is discarded in full and
  re-requested once, then given up on and logged. Never stored.
