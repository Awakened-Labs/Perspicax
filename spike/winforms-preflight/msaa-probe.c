/*
 * The pre-flight's instrument: does a WinForms window under Wine answer
 * WM_GETOBJECT with a real managed accessible tree, or does it fall through to
 * oleacc's stub path?
 *
 * MSAA, not UIA, and the distinction is load-bearing. The C1 kill criterion is
 * defined in UIA ControlType because uia-dump.exe reads UIA. This probe asks a
 * strictly earlier question -- whether a managed provider exists at all -- and
 * wine-mono's WinForms accessibility is IAccessible-shaped, which is what
 * Appendix 1 names. A "Live" verdict here is permission to write uia-dump.exe.
 * It is NOT the typed-node fraction, and must never be quoted as one.
 *
 * How the answer shows itself: a live provider returns real roles per control
 * (push button, editable text, check box, combo box, list, outline). An absent
 * one returns ROLE_SYSTEM_CLIENT for everything, which is the same signature
 * the Cross-Model table predicts for pure Win32 -- 27 of 28 oleacc control
 * classes stubbed, WC_EDITW the sole exception.
 */
#define COBJMACROS
#include <windows.h>
#include <oleacc.h>
#include <stdio.h>

static int total, typed;

static void role_name(long role, char *buf, int len)
{
    UINT n = GetRoleTextA((DWORD)role, buf, len);
    if (!n) snprintf(buf, len, "<role %ld>", role);
}

static void walk(IAccessible *acc, VARIANT vself, int depth)
{
    BSTR name = NULL;
    VARIANT vrole;
    char rbuf[128] = {0};
    long role = 0;

    VariantInit(&vrole);
    if (SUCCEEDED(IAccessible_get_accRole(acc, vself, &vrole)) && vrole.vt == VT_I4)
        role = vrole.lVal;
    role_name(role, rbuf, sizeof rbuf);
    IAccessible_get_accName(acc, vself, &name);

    total++;
    /* ROLE_SYSTEM_CLIENT is the generic fallback: a node oleacc could not
     * describe. Anything else was described by an actual provider. */
    if (role != ROLE_SYSTEM_CLIENT && role != 0) typed++;

    printf("%*s[%-22s] %ls\n", depth * 2, "", rbuf,
           name ? name : L"(no name)");
    if (name) SysFreeString(name);
    VariantClear(&vrole);

    if (depth > 6) return;

    long count = 0;
    if (FAILED(IAccessible_get_accChildCount(acc, &count)) || count <= 0) return;

    VARIANT *kids = calloc(count, sizeof *kids);
    long got = 0;
    if (kids && SUCCEEDED(AccessibleChildren(acc, 0, count, kids, &got))) {
        for (long i = 0; i < got; i++) {
            if (kids[i].vt == VT_DISPATCH) {
                IAccessible *child = NULL;
                if (SUCCEEDED(IDispatch_QueryInterface(kids[i].pdispVal,
                        &IID_IAccessible, (void **)&child)) && child) {
                    VARIANT self;
                    VariantInit(&self);
                    self.vt = VT_I4; self.lVal = CHILDID_SELF;
                    walk(child, self, depth + 1);
                    IAccessible_Release(child);
                }
            } else if (kids[i].vt == VT_I4) {
                walk(acc, kids[i], depth + 1);   /* a child-id leaf */
            }
            VariantClear(&kids[i]);
        }
    }
    free(kids);
}

static HWND found;
static BOOL CALLBACK pick(HWND hwnd, LPARAM lp)
{
    char title[256] = {0};
    GetWindowTextA(hwnd, title, sizeof title);
    if (strstr(title, (const char *)lp)) { found = hwnd; return FALSE; }
    return TRUE;
}

int main(int argc, char **argv)
{
    const char *want = argc > 1 ? argv[1] : "PreflightTarget";
    CoInitialize(NULL);

    for (int tries = 0; tries < 40 && !found; tries++) {
        EnumWindows(pick, (LPARAM)want);
        if (!found) Sleep(500);
    }
    if (!found) { fprintf(stderr, "no window whose title contains \"%s\"\n", want); return 2; }
    printf("window: %p  title contains \"%s\"\n\n", (void *)found, want);

    IAccessible *acc = NULL;
    HRESULT hr = AccessibleObjectFromWindow(found, OBJID_CLIENT,
                                            &IID_IAccessible, (void **)&acc);
    if (FAILED(hr) || !acc) {
        fprintf(stderr, "AccessibleObjectFromWindow failed: 0x%08lx\n", (unsigned long)hr);
        return 3;
    }

    VARIANT self;
    VariantInit(&self);
    self.vt = VT_I4; self.lVal = CHILDID_SELF;
    walk(acc, self, 0);
    IAccessible_Release(acc);

    printf("\nnodes: %d   non-generic roles: %d   generic (ROLE_SYSTEM_CLIENT): %d\n",
           total, typed, total - typed);
    puts(typed > 1 ? "VERDICT-SIGNAL: a provider described these nodes"
                   : "VERDICT-SIGNAL: generic fallback only -- no managed provider");
    CoUninitialize();
    return 0;
}
