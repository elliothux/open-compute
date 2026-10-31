# I56–57 actionable upgrades and synchronous stop

Issues #56 and #57 tighten the local operator lifecycle:

- upgrade operates only on registrations owned by the receipt executable;
- stopped registrations are not restart inputs, so a missing, changed, or invalid stopped config no longer blocks replacement. The plan reports its exact instance ID, canonical config path, stable error code, and `ocd instance unregister --instance <id>` recovery command;
- every invalid active registration is reported before any binary mutation, with the same exact identity and recovery command;
- `ocd stop` waits up to 30 seconds for three independent facts: the service manager reports inactive, the capability-scoped control socket no longer responds, and the platform lock is absent or can be exclusively inspected;
- macOS stop uses launchd `bootout` for the current session. `start` bootstraps an unloaded plist before kickstart, while the plist remains installed for the next login.

No asynchronous-success output or legacy stop mode remains. `INSTANCE_STOPPED` means an immediate offline operation or subsequent start is safe; timeout returns a failure instead.
