# Apple entitlements and capabilities

Team ID: **VUB229HYPU** (Individual) — Account holder: Mohamed Abdelsalam (mohamed@nazmi.ca)

| Entitlement / capability | Needed for | How obtained | Status | Date |
|---|---|---|---|---|
| Developer ID Application certificate | Signing/notarizing the app (spec §13) | Xcode → Settings → Accounts → Manage Certificates | **done** — "Developer ID Application: Mohamed Abdelsalam (VUB229HYPU)" | 2026-09-29 |
| Developer ID Installer certificate | Signing the `.pkg` (spec §13) | same | **done** | 2026-09-29 |
| `com.apple.developer.usb.host-controller-interface` | Generic USB redirection (spec §10.2) | Feedback Assistant request (managed entitlement) — text below | not submitted (user will submit) | |
| `com.apple.developer.system-extension.install` | Virtual camera system extension | Capability on the App ID (developer portal) | not started (Phase 4) | |
| FSKit module capability | Drive redirection backend (spec §10.2) | Capability on the extension's App ID | not started (Phase 4) | |

Private APIs (`CGVirtualDisplay`, `CGSCreateLoginSessionWithDataAndVisibility`, …) need no
entitlement but exclude Mac App Store distribution (spec §13).

## USB host-controller entitlement — Feedback Assistant request

| Field | Value |
|---|---|
| Platform | macOS |
| Descriptive Title | `Request for Entitlement — com.apple.developer.usb.host-controller-interface` |
| Problem Area | USB |
| Type of Feedback | Suggestion / Request |

Description (paste into "Describe the Issue"):

> Hello — I'd like to request access to the managed entitlement
> **`com.apple.developer.usb.host-controller-interface`** for my team.
>
> **Team ID:** VUB229HYPU
> **Developer:** Mohamed Abdelsalam (individual Apple Developer Program account)
>
> **Product:** a native **RDP server for macOS**: standard RDP clients (Microsoft
> Remote Desktop / Windows App / mstsc / FreeRDP) connect to a Mac and use its
> desktop — display, keyboard/mouse, audio, clipboard, drive and smart-card
> redirection. It targets headless Macs used remotely for development and photo work,
> and is distributed as a **Developer ID-signed, notarized** app (not via the Mac App
> Store). It builds on the open-source macrdp project.
>
> **Why I need the entitlement:** I'm implementing **USB device redirection** (the RDP
> `MS-RDPEUSB` feature). When a connected client redirects one of its local USB
> devices, the server must **present that device as a locally attached USB device on
> the Mac**, so standard macOS drivers and apps in the session can use it. I'm doing
> this in user space with **`IOUSBHostControllerInterface`** (driving
> `AppleUSBUserHCI`) — a virtual USB host controller fed with the redirected device's
> descriptors and transfers — rather than a kernel extension or a DriverKit
> `transport.usb` driver. That API requires this entitlement.
>
> **Scope:** embedded in a Developer ID provisioning profile, used only by this signed,
> notarized app. Happy to provide more detail.
>
> Thank you for considering the request.
>
> Mohamed Abdelsalam — mohamed@nazmi.ca
