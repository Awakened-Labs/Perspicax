# The wine-mono WinForms pre-flight

**The question.** Does wine-mono actually implement WinForms accessibility
(`AccessibleObject` / `IAccessible`)? If it does not — for the same reason WPF
was skipped — then C1 has no decision target and the project's two-gate
structure loses its second gate.

**Why it runs first.** Reviewer Concern #2: this is the cheapest thing in the
plan and it must precede a line of `uia-dump.exe`, not run alongside it.

**The budget.** Half a day. Measured on this box — Wine 11.4 staging (wow64),
wine-mono 11.0.0, kernel 6.12.58, X11.

## What this is NOT

**MSAA, not UIA, and the distinction is load-bearing.** C1's kill criterion is
defined in UIA `ControlType` because `uia-dump.exe` reads UIA. This pre-flight
asks a strictly earlier and cheaper question — *is there a managed provider at
all* — and wine-mono's WinForms accessibility is `IAccessible`-shaped, which is
what Appendix 1 names. **A "Live" verdict here is permission to write
`uia-dump.exe`. It is not the typed-node fraction and must never be quoted as
one.**

The target is also synthetic rather than the "small OSS WinForms app" Appendix 1
names. That is right for a pre-flight — it fixes the denominator exactly, one of
each interactive control, with nothing added after seeing the data — and wrong
for C1, which still needs a real application.

## The pieces

| | |
|---|---|
| `WinFormsTarget.cs` | one of each interactive control, built by the `csc.exe` inside the prefix |
| `msaa-probe.c` | walks the tree from the top-level window |
| `hwnd-diag.c` | asks each child control HWND directly — the one that found the answer |
| `Makefile` | mingw, stock PE, nothing Wine-specific |

## The trap that nearly produced a false negative

**Asking the top-level window answers almost nothing.** `AccessibleObjectFromWindow`
on the form returns `ROLE_SYSTEM_CLIENT` with **zero** children, which reads
exactly like "no provider" and would have ended the pre-flight at Absent.

It is wrong because WinForms answers `WM_GETOBJECT` **per control HWND**. The
tell that something was off: a generic oleacc client object *counts child HWNDs*,
and this one reported zero while the form demonstrably had eight. Walking
`EnumChildWindows` and asking each control is what produced the real tree.

## The discriminator

Roles alone cannot settle it. `WindowsForms10.EDIT` returning `text` proves
nothing — `WC_EDITW` is the **sole** non-stubbed class in Wine's oleacc
(`dlls/oleacc/client.c`, 27 of 28 stubbed), so oleacc's own handler explains it.

So the target sets two properties that exist **only** in managed code and that
oleacc cannot infer from a window class:

```csharp
AccessibleName = "MANAGED_SENTINEL",
AccessibleRole = AccessibleRole.Slider    // on a BUTTON-class window
```

Both came back. A `BUTTON`-class window reporting `role=slider` and
`accName="MANAGED_SENTINEL"` cannot be oleacc's stub path inferring anything —
a managed provider answered.

## Readings

Each child control HWND, asked directly:

| Control | Window class | Role | accName |
|---|---|---|---|
| MenuStrip | `WindowsForms10.Window.8` | **menu bar** | — |
| Button | `WindowsForms10.BUTTON` | **slider** ← managed | **MANAGED_SENTINEL** ← managed |
| TextBox | `WindowsForms10.EDIT` | **text** | — |
| CheckBox | `WindowsForms10.BUTTON` | **check button** | CheckMe |
| ComboBox | `WindowsForms10.COMBOBOX` | client | — |
| (combo's inner edit) | `Edit` | **text** | — |
| ListBox | `WindowsForms10.LISTBOX` | client | **MANAGED_LIST_SENTINEL** ← managed |
| TabControl | `WindowsForms10.SysTabControl32` | client | — |
| TabPage | `WindowsForms10.Window.8` | client | TabOne |
| TreeView | `WindowsForms10.SysTreeView32` | client | — |

Two `BUTTON`-class controls returning **different** roles is on its own enough to
rule out a purely class-driven stub table.

## Verdict

**Live, and partial.** wine-mono 11.0.0 implements WinForms accessibility: a
managed provider answers `WM_GETOBJECT` per control HWND, and it served both
sentinels — a role oleacc could never infer for a `BUTTON` window, and a name
that exists only in managed code. **C1 has a decision target, and
`uia-dump.exe` is worth writing.**

Partial is the more useful half of the finding. The provider answers, but role
coverage is uneven: Button, CheckBox, TextBox and MenuStrip resolve to real
roles, while ComboBox, ListBox, TabControl and TreeView fall back to
`ROLE_SYSTEM_CLIENT`. **ListBox is the precise case**, and it is why "partial"
rather than "mixed" — it returned the *managed name* and the *generic role* in
one answer, so the provider was reached and the role specifically was not
resolved. This is a provider that exists and is incomplete, not a provider that
is missing.

What that predicts for C1, stated now so it cannot be fitted to the data later:
the typed-node fraction on a real WinForms app will land **between** the Win32
floor and full semantics, and the kill criterion — fewer than half of
interactive controls typed — is genuinely live rather than a formality. On this
synthetic target 4 of 9 interactive controls carried a non-generic role, which
is close enough to the 50% line that the real measurement decides something.

**Measured on MSAA. Whether UIA's `ControlType` resolves where MSAA's role falls
back is exactly what C1 measures, and this pre-flight deliberately does not
guess at it.**

## One correction to the plan of record

The plan of record skips WPF on the grounds that wine-mono #223 leaves "15 WPF
DLLs absent". **In wine-mono 11.0.0 on this box they are present**: `PresentationFramework.dll` (6.5 MB), `PresentationCore.dll`
(3.9 MB) and `WindowsBase.dll` (1.2 MB) are all in the GAC.

Present is not the same as working, and this pre-flight did not test a WPF app —
so this retires the *stated reason* for skipping WPF, not the decision. If C1
wants a second target, WPF is worth ten minutes of re-checking rather than
being ruled out by a citation that no longer describes the shipped assemblies.
