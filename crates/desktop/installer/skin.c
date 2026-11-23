#define WIN32_LEAN_AND_MEAN
#define UNICODE
#define _UNICODE
#include <windows.h>
#include <windowsx.h>
#include <commctrl.h>

static const COLORREF surface = RGB(247, 249, 253);
static const COLORREF ink = RGB(19, 38, 74);
static const COLORREF blue = RGB(23, 97, 231);
static const COLORREF white = RGB(255, 255, 255);

enum SkinRole { PRIMARY = 1, SECONDARY, EDIT, CHECKBOX, MINIMIZE, CLOSE, RADIO };
enum { HOVER = 16 };

static int dpi_px(HWND window, int size) {
    return MulDiv(size, GetDpiForWindow(window), 96);
}

static BOOL high_contrast(void) {
    HIGHCONTRASTW settings = { sizeof(settings) };
    return SystemParametersInfoW(SPI_GETHIGHCONTRAST, sizeof(settings), &settings, 0)
        && (settings.dwFlags & HCF_HIGHCONTRASTON);
}

static void fill(HDC dc, const RECT *rect, COLORREF color) {
    HBRUSH brush = CreateSolidBrush(color);
    FillRect(dc, rect, brush);
    DeleteObject(brush);
}

static COLORREF caption_color(int x, int width) {
    return RGB(12 + MulDiv(9, x, width), 32 + MulDiv(61, x, width), 72 + MulDiv(132, x, width));
}

static void caption_background(HWND window, HDC dc, const RECT *rect, int left) {
    if (high_contrast()) {
        fill(dc, rect, GetSysColor(COLOR_WINDOW));
        return;
    }
    RECT parent;
    GetClientRect(GetParent(window), &parent);
    int width = max(1, parent.right);
    for (int x = rect->left; x < rect->right; x++) {
        RECT column = { x, rect->top, x + 1, rect->bottom };
        fill(dc, &column, caption_color(left + x, width));
    }
}

static void rounded(HDC dc, const RECT *rect, int radius, COLORREF bg, COLORREF edge) {
    HPEN pen = CreatePen(PS_SOLID, 1, edge);
    HBRUSH brush = CreateSolidBrush(bg);
    HGDIOBJ old_pen = SelectObject(dc, pen);
    HGDIOBJ old_brush = SelectObject(dc, brush);
    RoundRect(dc, rect->left, rect->top, rect->right, rect->bottom, radius, radius);
    SelectObject(dc, old_brush);
    SelectObject(dc, old_pen);
    DeleteObject(brush);
    DeleteObject(pen);
}

static void stroke(HDC dc, int x, int y, int xx, int yy) {
    MoveToEx(dc, x, y, NULL);
    LineTo(dc, xx, yy);
}

static void draw_button(HWND window, HDC dc, UINT role, BOOL hover) {
    RECT rect;
    GetClientRect(window, &rect);
    unsigned state = (unsigned)SendMessageW(window, BM_GETSTATE, 0, 0);
    BOOL down = (state & BST_PUSHED) != 0;
    BOOL enabled = IsWindowEnabled(window);
    BOOL focused = GetFocus() == window;
    BOOL caption = role == MINIMIZE || role == CLOSE;
    BOOL contrast = high_contrast();
    COLORREF background = surface;
    COLORREF foreground = enabled ? ink : RGB(97, 113, 138);

    if (contrast) {
        background = GetSysColor(COLOR_WINDOW);
        foreground = GetSysColor(enabled ? COLOR_WINDOWTEXT : COLOR_GRAYTEXT);
    }
    if (caption) {
        RECT position;
        GetWindowRect(window, &position);
        MapWindowPoints(NULL, GetParent(window), (POINT *)&position, 2);
        caption_background(window, dc, &rect, position.left);
    } else {
        fill(dc, &rect, background);
    }

    if (caption) {
        if (enabled && (hover || focused)) {
            COLORREF highlight = contrast ? GetSysColor(COLOR_HIGHLIGHT)
                : role == CLOSE ? (down ? RGB(152, 31, 49) : RGB(193, 47, 65))
                : (down ? RGB(21, 57, 120) : RGB(31, 71, 140));
            rounded(dc, &rect, dpi_px(window, 7), highlight, highlight);
        }
        COLORREF icon = contrast
            ? GetSysColor(enabled && (hover || focused) ? COLOR_HIGHLIGHTTEXT : enabled ? COLOR_WINDOWTEXT : COLOR_GRAYTEXT)
            : enabled ? white : RGB(131, 153, 189);
        HPEN pen = CreatePen(PS_SOLID, dpi_px(window, 1) + 1, icon);
        HGDIOBJ old = SelectObject(dc, pen);
        int x = rect.right / 2, y = rect.bottom / 2, half = dpi_px(window, 5);
        if (role == MINIMIZE) {
            stroke(dc, x - half, y + half - 1, x + half + 1, y + half - 1);
        } else {
            stroke(dc, x - half, y - half, x + half + 1, y + half + 1);
            stroke(dc, x - half, y + half, x + half + 1, y - half - 1);
        }
        SelectObject(dc, old);
        DeleteObject(pen);
    } else {
        RECT text = rect;
        BOOL choice = role == CHECKBOX || role == RADIO;
        if (choice) {
            int size = dpi_px(window, 16);
            int y = (rect.bottom - size) / 2;
            RECT box = { 0, y, size, y + size };
            BOOL checked = SendMessageW(window, BM_GETCHECK, 0, 0) == BST_CHECKED;
            COLORREF active = contrast ? GetSysColor(COLOR_HIGHLIGHT) : blue;
            COLORREF edge = checked || hover ? active : contrast ? foreground : RGB(148, 168, 199);
            if (role == RADIO) {
                rounded(dc, &box, size, background, edge);
                if (checked) {
                    InflateRect(&box, -size / 4, -size / 4);
                    rounded(dc, &box, size, active, active);
                }
            } else {
                rounded(dc, &box, dpi_px(window, 5), checked ? active : background, edge);
            }
            if (checked && role == CHECKBOX) {
                HPEN pen = CreatePen(PS_SOLID, dpi_px(window, 2), contrast ? GetSysColor(COLOR_HIGHLIGHTTEXT) : white);
                HGDIOBJ old = SelectObject(dc, pen);
                stroke(dc, size / 5, y + size / 2, size * 2 / 5, y + size * 3 / 4);
                stroke(dc, size * 2 / 5, y + size * 3 / 4, size * 4 / 5, y + size / 4);
                SelectObject(dc, old);
                DeleteObject(pen);
            }
            text.left += size + dpi_px(window, 8);
        } else {
            COLORREF bg = white, edge = RGB(197, 211, 234);
            if (contrast) {
                bg = GetSysColor(COLOR_BTNFACE);
                edge = GetSysColor(COLOR_BTNTEXT);
            } else if (!enabled) {
                bg = RGB(235, 240, 249);
                edge = bg;
            } else if (role == PRIMARY) {
                bg = down ? RGB(17, 68, 179) : hover ? RGB(18, 80, 203) : blue;
                edge = bg;
                foreground = white;
            } else if (hover || down) {
                bg = down ? RGB(222, 233, 251) : RGB(237, 244, 255);
                edge = RGB(130, 168, 232);
            }
            rounded(dc, &rect, dpi_px(window, 9), bg, edge);
        }

        WCHAR label[256];
        GetWindowTextW(window, label, ARRAYSIZE(label));
        HFONT font = (HFONT)SendMessageW(window, WM_GETFONT, 0, 0);
        HGDIOBJ old_font = font ? SelectObject(dc, font) : NULL;
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, foreground);
        DrawTextW(dc, label, -1, &text, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS
                  | (choice ? DT_LEFT : DT_CENTER)
                  | ((SendMessageW(window, WM_QUERYUISTATE, 0, 0) & UISF_HIDEACCEL) ? DT_HIDEPREFIX : 0));
        if (old_font) SelectObject(dc, old_font);
    }
    if (focused && !(SendMessageW(window, WM_QUERYUISTATE, 0, 0) & UISF_HIDEFOCUS)) {
        InflateRect(&rect, -dpi_px(window, 3), -dpi_px(window, 3));
        DrawFocusRect(dc, &rect);
    }
}

static LRESULT CALLBACK button_proc(HWND window, UINT msg, WPARAM w, LPARAM l, UINT_PTR id, DWORD_PTR value) {
    if (msg == WM_PAINT) {
        PAINTSTRUCT ps;
        BeginPaint(window, &ps);
        draw_button(window, ps.hdc, (UINT)value & 15, (value & HOVER) != 0);
        EndPaint(window, &ps);
        return 0;
    }
    if (msg == WM_ERASEBKGND) return 1;
    LRESULT result = DefSubclassProc(window, msg, w, l);
    switch (msg) {
    case WM_MOUSEMOVE:
        if (!(value & HOVER)) {
            TRACKMOUSEEVENT tracking = { sizeof(tracking), TME_LEAVE, window, 0 };
            TrackMouseEvent(&tracking);
            SetWindowSubclass(window, button_proc, id, value | HOVER);
            InvalidateRect(window, NULL, FALSE);
        }
        break;
    case WM_MOUSELEAVE:
        SetWindowSubclass(window, button_proc, id, value & ~HOVER);
        InvalidateRect(window, NULL, FALSE);
        break;
    case WM_ENABLE: case WM_SETFOCUS: case WM_KILLFOCUS: case WM_SETTEXT: case WM_SETFONT:
    case BM_SETCHECK: case BM_SETSTATE: case WM_UPDATEUISTATE: case WM_THEMECHANGED:
        InvalidateRect(window, NULL, FALSE);
        break;
    case WM_NCDESTROY:
        RemoveWindowSubclass(window, button_proc, id);
        break;
    }
    return result;
}

static LRESULT CALLBACK edit_proc(HWND window, UINT msg, WPARAM w, LPARAM l, UINT_PTR id, DWORD_PTR value) {
    (void)value;
    if (msg == WM_NCCALCSIZE) {
        RECT *rect = w ? &((NCCALCSIZE_PARAMS *)l)->rgrc[0] : (RECT *)l;
        int inset = dpi_px(window, 3);
        rect->left += inset;
        rect->right -= inset;
        rect->top += inset;
        rect->bottom -= inset;
        return 0;
    }
    if (msg == WM_NCPAINT) {
        HDC dc = GetWindowDC(window);
        if (dc) {
            RECT rect, client;
            POINT pos = { 0, 0 };
            GetWindowRect(window, &rect);
            ClientToScreen(window, &pos);
            GetClientRect(window, &client);
            int dx = pos.x - rect.left, dy = pos.y - rect.top;
            OffsetRect(&rect, -rect.left, -rect.top);
            ExcludeClipRect(dc, dx, dy, dx + client.right, dy + client.bottom);
            COLORREF border = high_contrast() ? GetSysColor(COLOR_WINDOWTEXT)
                : GetFocus() == window ? blue : RGB(188, 205, 234);
            fill(dc, &rect, high_contrast() ? GetSysColor(COLOR_WINDOW) : surface);
            rounded(dc, &rect, dpi_px(window, 8), high_contrast() ? GetSysColor(COLOR_WINDOW) : white, border);
            ReleaseDC(window, dc);
        }
        return 0;
    }
    LRESULT result = DefSubclassProc(window, msg, w, l);
    if (msg == WM_SETFOCUS || msg == WM_KILLFOCUS)
        RedrawWindow(window, NULL, NULL, RDW_FRAME | RDW_INVALIDATE);
    if (msg == WM_NCDESTROY) RemoveWindowSubclass(window, edit_proc, id);
    return result;
}

static void skin(HWND window, UINT role) {
    if (!window) return;
    if (role == EDIT) {
        SetWindowLongPtrW(window, GWL_STYLE, GetWindowLongPtrW(window, GWL_STYLE) & ~WS_BORDER);
        SetWindowLongPtrW(window, GWL_EXSTYLE, GetWindowLongPtrW(window, GWL_EXSTYLE) & ~WS_EX_CLIENTEDGE);
        SetWindowSubclass(window, edit_proc, 1, 0);
        int padding = dpi_px(window, 6);
        SendMessageW(window, EM_SETMARGINS, EC_LEFTMARGIN | EC_RIGHTMARGIN, MAKELPARAM(padding, padding));
        SetWindowPos(window, NULL, 0, 0, 0, 0,
                     SWP_NOSIZE | SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    } else {
        SetWindowSubclass(window, button_proc, 1, role);
    }
    RedrawWindow(window, NULL, NULL, RDW_FRAME | RDW_INVALIDATE);
}

enum { TITLE_STRIP = 3010, TITLE_MIN = 3011, TITLE_CLOSE = 3012 };

static LRESULT CALLBACK title_proc(HWND window, UINT msg, WPARAM w, LPARAM l, UINT_PTR id, DWORD_PTR value) {
    (void)value;
    if (msg == WM_PAINT) {
        PAINTSTRUCT ps;
        RECT rect;
        BeginPaint(window, &ps);
        GetClientRect(window, &rect);
        caption_background(window, ps.hdc, &rect, 0);
        EndPaint(window, &ps);
        return 0;
    }
    if (msg == WM_LBUTTONDOWN) {
        POINT point;
        GetCursorPos(&point);
        ReleaseCapture();
        SendMessageW(GetAncestor(window, GA_ROOT), WM_NCLBUTTONDOWN, HTCAPTION, MAKELPARAM(point.x, point.y));
        return 0;
    }
    if (msg == WM_NCDESTROY) RemoveWindowSubclass(window, title_proc, id);
    return DefSubclassProc(window, msg, w, l);
}

static LRESULT CALLBACK window_proc(HWND window, UINT msg, WPARAM w, LPARAM l, UINT_PTR id, DWORD_PTR value) {
    if (msg == WM_COMMAND && HIWORD(w) == BN_CLICKED) {
        HWND root = GetAncestor(window, GA_ROOT);
        if (LOWORD(w) == TITLE_MIN) {
            ShowWindow(root, SW_MINIMIZE);
            return 0;
        }
        if (LOWORD(w) == TITLE_CLOSE) {
            if (value) {
                HWND child = GetDlgItem(window, 1203);
                SendMessageW(child, BM_SETCHECK, BST_UNCHECKED, 0);
                child = GetDlgItem(window, 1204);
                SendMessageW(child, BM_SETCHECK, BST_UNCHECKED, 0);
                SendMessageW(GetDlgItem(root, IDOK), BM_CLICK, 0, 0);
            } else if (IsWindowEnabled(GetDlgItem(root, IDCANCEL))) {
                SendMessageW(root, WM_CLOSE, 0, 0);
            }
            return 0;
        }
    }
    if (msg == WM_NCDESTROY) RemoveWindowSubclass(window, window_proc, id);
    return DefSubclassProc(window, msg, w, l);
}

__declspec(dllexport) void __cdecl ApplyControl(HWND window, UINT role) {
    skin(window, role);
}

__declspec(dllexport) void __cdecl SetupCaption(HWND window) {
    RECT outer, before, after;
    GetWindowRect(window, &outer);
    GetClientRect(window, &before);
    LONG_PTR style = GetWindowLongPtrW(window, GWL_STYLE);
    SetWindowLongPtrW(window, GWL_STYLE, (style & ~WS_CAPTION) | WS_MINIMIZEBOX);
    SetWindowPos(window, NULL, 0, 0, 0, 0,
                 SWP_NOSIZE | SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    GetClientRect(window, &after);
    SetWindowPos(window, NULL, 0, 0, outer.right - outer.left - (after.right - before.right),
                 outer.bottom - outer.top - (after.bottom - before.bottom), SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
}

__declspec(dllexport) void __cdecl PositionCaption(HWND window, HWND page, BOOL finishing) {
    if (!page || GetDlgItem(page, TITLE_STRIP)) return;
    SetWindowSubclass(page, window_proc, 1, finishing);
    HWND strip = CreateWindowExW(0, L"Static", L"", WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | SS_NOTIFY,
                                 0, 0, 0, 0, page, (HMENU)TITLE_STRIP, NULL, NULL);
    SetWindowSubclass(strip, title_proc, 1, 0);
    HWND min = CreateWindowExW(0, L"Button", L"\x6700\x5c0f\x5316", WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_CLIPSIBLINGS,
                               0, 0, 0, 0, page, (HMENU)TITLE_MIN, NULL, NULL);
    HWND close = CreateWindowExW(0, L"Button", L"\x5173\x95ed", WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_CLIPSIBLINGS,
                                 0, 0, 0, 0, page, (HMENU)TITLE_CLOSE, NULL, NULL);
    skin(min, MINIMIZE);
    skin(close, CLOSE);

    RECT rect;
    GetClientRect(window, &rect);
    int button = dpi_px(window, 35), gap = dpi_px(window, 5), height = dpi_px(window, 29);
    EnableWindow(close, finishing || IsWindowEnabled(GetDlgItem(window, IDCANCEL)));
    SetWindowPos(strip, HWND_TOP, 0, 0, rect.right, height + gap, SWP_NOACTIVATE);
    SetWindowPos(min, HWND_TOP, rect.right - (button + gap) * 2, gap / 2, button, height, SWP_NOACTIVATE);
    SetWindowPos(close, HWND_TOP, rect.right - button - gap, gap / 2, button, height, SWP_NOACTIVATE);
}

__declspec(dllexport) void __cdecl FillClientWidth(HWND window, HWND child) {
    RECT parent, rect;
    GetClientRect(window, &parent);
    GetWindowRect(child, &rect);
    SetWindowPos(child, NULL, 0, 0, parent.right, rect.bottom - rect.top,
                 SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOZORDER);
}
