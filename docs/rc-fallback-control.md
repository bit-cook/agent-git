# Codex fallback control

RC prefers the selected runtime source's persistent app-server socket. Closing a
subscriber does not own that service's lifetime, and shared connections have no
inactivity release timer. An unavailable or invalid existing listener is an error;
it must not silently create a competing executor.

When the selected executable explicitly lacks an app-server listener, RC can use
a private app-server in that same runtime home. After acquiring the conversation,
the fallback controller tracks human instructions. Reading history, inspecting
settings, model output, connection loss, and creation-receipt retries do not renew
the timer. The default deadline is 15 minutes.

At the deadline, RC first requires a readable transcript and native configuration
readback confirming bounded idle thread unloading. It then calls
`thread/unsubscribe`, keeping the app-server process and running task alive.
Native execution retains its writer while the task runs; the native idle unload
releases that writer after the task finishes. This operation never interrupts a
turn or terminates the process.

RC continues reading saved transcript output and polls read-only native status.
An approval that appears in the background is reported as waiting. Model and
permission observations come from the selected runtime home, and missing evidence
is represented as unknown rather than retaining an earlier owner's settings.

The next instruction, or **Resume control**, first resumes the exact native thread
without overriding its model or permissions. RC reconciles active turn identity
and rechecks current caller authority before sending input. A competing native
writer or uncertain resume result leaves the task in observation mode and returns
an error without sending the instruction. Resume control itself sends no message.

The `session.model` response includes `control.mode`: `shared`, `fallback`,
`background`, `unknown`, or `fallback_release_unavailable` for established
conversations. Unknown release outcomes do not claim success or stop the task.
Unsupported native unloading keeps control attached and reports the limitation.
In particular, Codex 0.153 does not expose the configurable bounded unload used
here; the non-stopping release path has been exercised with the native
0.155.0-alpha.16.4 app-server. Runtime capability readback, rather than a version
string, decides whether release is available.

The maintained tests cover the intent deadline, shared-mode exclusion, unreadable
history, uncertain release, source-specific settings, current policy admission,
and completion reconciliation. An opt-in isolated native test also verifies that
work continues, a concurrent writer is refused while it runs, another executor
can acquire the completed conversation, and RC can subsequently reacquire it.
These local checks do not establish remote latency or production rollout.
