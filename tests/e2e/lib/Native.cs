// Native helpers for the end-to-end runner: real mouse and keyboard input (SendInput), window placement and focus,
// PrintWindow captures, and a clipboard backup. Compiled by Add-Type in Windows PowerShell 5.1, so this is C# 5:
// no string interpolation, no `?.`, no expression-bodied members.
//
// The process is made DPI aware (per monitor v2) before anything else, so UI Automation rectangles, GetWindowRect
// and the input coordinates all use the same physical pixels. At 125% scale the numbers differ from what a
// non-aware process sees, and a click lands off target.

using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Forms;

namespace GlintE2E
{
    public struct WinInfo
    {
        public IntPtr Handle;
        public string Title;
        public string Class;
        public bool Visible;
        public int Left, Top, Right, Bottom;
    }

    public static class Win
    {
        [StructLayout(LayoutKind.Sequential)]
        struct RECT { public int Left, Top, Right, Bottom; }

        [StructLayout(LayoutKind.Sequential)]
        struct MOUSEINPUT
        {
            public int dx, dy;
            public uint mouseData, dwFlags, time;
            public UIntPtr dwExtraInfo;
        }

        [StructLayout(LayoutKind.Sequential)]
        struct KEYBDINPUT
        {
            public ushort wVk, wScan;
            public uint dwFlags, time;
            public UIntPtr dwExtraInfo;
        }

        [StructLayout(LayoutKind.Explicit)]
        struct InputUnion
        {
            [FieldOffset(0)] public MOUSEINPUT mi;
            [FieldOffset(0)] public KEYBDINPUT ki;
        }

        [StructLayout(LayoutKind.Sequential)]
        struct INPUT { public uint type; public InputUnion U; }

        delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lParam);

        [DllImport("user32.dll")] static extern uint SendInput(uint n, INPUT[] inputs, int size);
        [DllImport("user32.dll")] static extern bool SetProcessDpiAwarenessContext(IntPtr context);
        [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
        [DllImport("user32.dll")] static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lParam);
        [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
        [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr hwnd, StringBuilder sb, int max);
        [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr hwnd, StringBuilder sb, int max);
        [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hwnd);
        [DllImport("user32.dll")] static extern bool IsWindow(IntPtr hwnd);
        [DllImport("user32.dll")] static extern bool IsIconic(IntPtr hwnd);
        [DllImport("user32.dll")] static extern bool IsZoomed(IntPtr hwnd);
        [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
        [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr hwnd, IntPtr after, int x, int y, int w, int h, uint flags);
        [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr hwnd, int cmd);
        [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
        [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr hwnd);
        [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr hwnd);
        [DllImport("user32.dll")] static extern bool AttachThreadInput(uint from, uint to, bool attach);
        [DllImport("user32.dll")] static extern bool PostMessage(IntPtr hwnd, uint msg, IntPtr w, IntPtr l);
        [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
        [DllImport("user32.dll")] static extern uint MapVirtualKey(uint code, uint mapType);
        [DllImport("user32.dll")] static extern short GetAsyncKeyState(int vk);
        [DllImport("user32.dll")] static extern bool GetCursorPos(out POINT pt);
        [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(POINT pt);
        [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr hwnd, uint flags);
        [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();

        [StructLayout(LayoutKind.Sequential)]
        struct POINT { public int X, Y; }

        const uint INPUT_MOUSE = 0, INPUT_KEYBOARD = 1;
        const uint MOUSEEVENTF_MOVE = 0x1, MOUSEEVENTF_LEFTDOWN = 0x2, MOUSEEVENTF_LEFTUP = 0x4,
            MOUSEEVENTF_RIGHTDOWN = 0x8, MOUSEEVENTF_RIGHTUP = 0x10, MOUSEEVENTF_WHEEL = 0x800,
            MOUSEEVENTF_VIRTUALDESK = 0x4000, MOUSEEVENTF_ABSOLUTE = 0x8000;
        const uint KEYEVENTF_EXTENDEDKEY = 0x1, KEYEVENTF_KEYUP = 0x2, KEYEVENTF_UNICODE = 0x4;
        const uint SWP_NOZORDER = 0x4, SWP_NOACTIVATE = 0x10;
        const uint WM_CLOSE = 0x10;

        static bool dpiSet;

        /// Makes this process per-monitor-v2 DPI aware. Call it before touching UI Automation.
        public static void DpiAware()
        {
            if (dpiSet) return;
            dpiSet = true;
            try { SetProcessDpiAwarenessContext(new IntPtr(-4)); } catch (Exception) { }
        }

        // ---------- windows ----------

        public static List<WinInfo> TopWindows(int pid)
        {
            List<WinInfo> found = new List<WinInfo>();
            EnumWindows(delegate(IntPtr h, IntPtr l)
            {
                uint p;
                GetWindowThreadProcessId(h, out p);
                if (pid != 0 && p != (uint)pid) return true;
                found.Add(Describe(h));
                return true;
            }, IntPtr.Zero);
            return found;
        }

        /// Top-level windows of any process whose class is `className`.
        public static List<WinInfo> WindowsOfClass(string className)
        {
            List<WinInfo> found = new List<WinInfo>();
            EnumWindows(delegate(IntPtr h, IntPtr l)
            {
                StringBuilder c = new StringBuilder(256);
                GetClassName(h, c, 256);
                if (c.ToString() == className) found.Add(Describe(h));
                return true;
            }, IntPtr.Zero);
            return found;
        }

        public static WinInfo Describe(IntPtr h)
        {
            WinInfo w = new WinInfo();
            w.Handle = h;
            StringBuilder t = new StringBuilder(512);
            GetWindowText(h, t, 512);
            StringBuilder c = new StringBuilder(256);
            GetClassName(h, c, 256);
            w.Title = t.ToString();
            w.Class = c.ToString();
            w.Visible = IsWindowVisible(h);
            RECT r;
            if (GetWindowRect(h, out r)) { w.Left = r.Left; w.Top = r.Top; w.Right = r.Right; w.Bottom = r.Bottom; }
            return w;
        }

        public static bool Exists(IntPtr h) { return IsWindow(h); }
        public static bool Maximized(IntPtr h) { return IsZoomed(h); }
        public static IntPtr Foreground() { return GetForegroundWindow(); }

        public static int[] Rect(IntPtr h)
        {
            RECT r;
            if (!GetWindowRect(h, out r)) return new int[] { 0, 0, 0, 0 };
            return new int[] { r.Left, r.Top, r.Right, r.Bottom };
        }

        public static void Place(IntPtr h, int x, int y, int w, int hgt)
        {
            if (IsZoomed(h) || IsIconic(h)) ShowWindow(h, 9);
            SetWindowPos(h, IntPtr.Zero, x, y, w, hgt, SWP_NOZORDER | SWP_NOACTIVATE);
        }

        /// Posts WM_CLOSE (what the close button does).
        public static void Close(IntPtr h) { PostMessage(h, WM_CLOSE, IntPtr.Zero, IntPtr.Zero); }

        /// Brings `h` to the front so SendInput reaches it. True if it is the foreground window afterwards.
        public static bool Focus(IntPtr h)
        {
            if (IsIconic(h)) ShowWindow(h, 9);
            for (int attempt = 0; attempt < 5; attempt++)
            {
                if (GetForegroundWindow() == h) return true;
                // A bare Alt press lets SetForegroundWindow through when this process isn't the foreground one.
                keybd(0x12, false);
                keybd(0x12, true);
                IntPtr fg = GetForegroundWindow();
                uint pid;
                uint fgThread = fg == IntPtr.Zero ? 0 : GetWindowThreadProcessId(fg, out pid);
                uint me = GetCurrentThreadId();
                bool attached = fgThread != 0 && fgThread != me && AttachThreadInput(me, fgThread, true);
                BringWindowToTop(h);
                SetForegroundWindow(h);
                if (attached) AttachThreadInput(me, fgThread, false);
                Thread.Sleep(120);
            }
            return GetForegroundWindow() == h;
        }

        /// The top-level window under a screen point (0 if none).
        public static IntPtr TopWindowAt(int x, int y)
        {
            POINT p; p.X = x; p.Y = y;
            IntPtr h = WindowFromPoint(p);
            if (h == IntPtr.Zero) return h;
            IntPtr root = GetAncestor(h, 2);
            return root == IntPtr.Zero ? h : root;
        }

        // ---------- capture ----------

        /// Saves a PNG of the window: PrintWindow first, and the screen's pixels when that comes back all black
        /// (a GPU-drawn window sometimes does). Returns how it was done, or "" on failure.
        public static string Capture(IntPtr h, string path)
        {
            try
            {
                RECT r;
                if (!GetWindowRect(h, out r)) return "";
                int w = r.Right - r.Left, hgt = r.Bottom - r.Top;
                if (w <= 0 || hgt <= 0) return "";
                using (Bitmap bmp = new Bitmap(w, hgt, PixelFormat.Format32bppArgb))
                {
                    bool ok;
                    using (Graphics g = Graphics.FromImage(bmp))
                    {
                        IntPtr hdc = g.GetHdc();
                        ok = PrintWindow(h, hdc, 2);
                        g.ReleaseHdc(hdc);
                    }
                    string how = "PrintWindow";
                    if (!ok || LooksBlank(bmp))
                    {
                        using (Graphics g = Graphics.FromImage(bmp))
                        {
                            g.CopyFromScreen(r.Left, r.Top, 0, 0, new Size(w, hgt));
                        }
                        how = "screen copy";
                    }
                    bmp.Save(path, ImageFormat.Png);
                    return how;
                }
            }
            catch (Exception) { return ""; }
        }

        /// Saves a PNG of the whole virtual screen (what a person would see), or returns "" on failure.
        public static string CaptureScreen(string path)
        {
            try
            {
                int vx = GetSystemMetrics(76), vy = GetSystemMetrics(77), vw = GetSystemMetrics(78), vh = GetSystemMetrics(79);
                using (Bitmap bmp = new Bitmap(vw, vh, PixelFormat.Format32bppArgb))
                {
                    using (Graphics g = Graphics.FromImage(bmp))
                    {
                        g.CopyFromScreen(vx, vy, 0, 0, new Size(vw, vh));
                    }
                    bmp.Save(path, ImageFormat.Png);
                }
                return "screen";
            }
            catch (Exception) { return ""; }
        }

        /// Presses the left button at (x1,y1), moves to (x2,y2) in small steps and stops there with the button still
        /// down. Pair with ButtonUp. For experiments that look at the screen mid-drag.
        public static void DragHold(int x1, int y1, int x2, int y2, int steps, int stepMs)
        {
            MoveTo(x1, y1);
            Thread.Sleep(80);
            ButtonDown(false);
            Thread.Sleep(120);
            for (int s = 1; s <= steps; s++)
            {
                MoveTo(x1 + (int)Math.Round((x2 - x1) * (double)s / steps), y1 + (int)Math.Round((y2 - y1) * (double)s / steps));
                Thread.Sleep(stepMs);
            }
            Thread.Sleep(300);
        }

        static bool LooksBlank(Bitmap bmp)
        {
            int stepX = Math.Max(1, bmp.Width / 24), stepY = Math.Max(1, bmp.Height / 24);
            for (int y = 0; y < bmp.Height; y += stepY)
                for (int x = 0; x < bmp.Width; x += stepX)
                {
                    Color c = bmp.GetPixel(x, y);
                    if (c.R > 8 || c.G > 8 || c.B > 8) return false;
                }
            return true;
        }

        // ---------- mouse ----------

        static void Mouse(uint flags, int dx, int dy, int data)
        {
            INPUT[] i = new INPUT[1];
            i[0].type = INPUT_MOUSE;
            i[0].U.mi.dx = dx; i[0].U.mi.dy = dy; i[0].U.mi.mouseData = (uint)data; i[0].U.mi.dwFlags = flags;
            SendInput(1, i, Marshal.SizeOf(typeof(INPUT)));
        }

        /// Moves the pointer to a physical screen pixel.
        public static void MoveTo(int x, int y)
        {
            int vx = GetSystemMetrics(76), vy = GetSystemMetrics(77), vw = GetSystemMetrics(78), vh = GetSystemMetrics(79);
            int nx = (int)Math.Round((x - vx) * 65535.0 / Math.Max(1, vw - 1));
            int ny = (int)Math.Round((y - vy) * 65535.0 / Math.Max(1, vh - 1));
            Mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, nx, ny, 0);
        }

        public static int[] Cursor()
        {
            POINT p;
            GetCursorPos(out p);
            return new int[] { p.X, p.Y };
        }

        public static void ButtonDown(bool right) { Mouse(right ? MOUSEEVENTF_RIGHTDOWN : MOUSEEVENTF_LEFTDOWN, 0, 0, 0); }
        public static void ButtonUp(bool right) { Mouse(right ? MOUSEEVENTF_RIGHTUP : MOUSEEVENTF_LEFTUP, 0, 0, 0); }

        public static void Click(int x, int y, bool right)
        {
            MoveTo(x, y);
            Thread.Sleep(60);
            ButtonDown(right);
            Thread.Sleep(40);
            ButtonUp(right);
            Thread.Sleep(80);
        }

        public static void DoubleClick(int x, int y)
        {
            MoveTo(x, y);
            Thread.Sleep(60);
            for (int n = 0; n < 2; n++)
            {
                ButtonDown(false);
                Thread.Sleep(30);
                ButtonUp(false);
                Thread.Sleep(50);
            }
            Thread.Sleep(80);
        }

        public static void Wheel(int x, int y, int notches)
        {
            MoveTo(x, y);
            Thread.Sleep(40);
            Mouse(MOUSEEVENTF_WHEEL, 0, 0, notches * 120);
            Thread.Sleep(80);
        }

        /// Presses at (x1,y1), moves in `steps` small moves with `stepMs` between them, holds at the end for
        /// `holdMs` (so spring-loaded targets and OLE drop targets see the pointer arrive), and releases.
        /// `key` (a virtual key such as Shift, or 0) is pressed once the drag is under way and released after the
        /// drop: a modifier held at the press would change the selection instead of carrying the item.
        public static void Drag(int x1, int y1, int x2, int y2, int steps, int stepMs, int holdMs, ushort key)
        {
            MoveTo(x1, y1);
            Thread.Sleep(80);
            ButtonDown(false);
            Thread.Sleep(120);
            for (int s = 1; s <= steps; s++)
            {
                int x = x1 + (int)Math.Round((x2 - x1) * (double)s / steps);
                int y = y1 + (int)Math.Round((y2 - y1) * (double)s / steps);
                MoveTo(x, y);
                Thread.Sleep(stepMs);
                if (key != 0 && s == Math.Min(3, steps)) { KeyDown(key); }
            }
            MoveTo(x2, y2);
            Thread.Sleep(holdMs);
            MoveTo(x2 + 1, y2);
            Thread.Sleep(60);
            MoveTo(x2, y2);
            Thread.Sleep(60);
            ButtonUp(false);
            Thread.Sleep(150);
            if (key != 0) { KeyUp(key); }
            Thread.Sleep(150);
        }

        // ---------- keyboard ----------

        static bool IsExtended(ushort vk)
        {
            switch (vk)
            {
                case 0x21: case 0x22: case 0x23: case 0x24: // PgUp PgDn End Home
                case 0x25: case 0x26: case 0x27: case 0x28: // arrows
                case 0x2D: case 0x2E: case 0x5D: case 0x6F: case 0x90: case 0xA3: case 0xA5:
                    return true;
            }
            return false;
        }

        static void keybd(ushort vk, bool up)
        {
            INPUT[] i = new INPUT[1];
            i[0].type = INPUT_KEYBOARD;
            i[0].U.ki.wVk = vk;
            i[0].U.ki.wScan = (ushort)MapVirtualKey(vk, 0);
            i[0].U.ki.dwFlags = (IsExtended(vk) ? KEYEVENTF_EXTENDEDKEY : 0) | (up ? KEYEVENTF_KEYUP : 0);
            SendInput(1, i, Marshal.SizeOf(typeof(INPUT)));
        }

        public static void KeyDown(ushort vk) { keybd(vk, false); Thread.Sleep(15); }
        public static void KeyUp(ushort vk) { keybd(vk, true); Thread.Sleep(15); }

        /// Presses `keys` together (modifiers first), then releases them in reverse.
        public static void Chord(ushort[] keys)
        {
            for (int n = 0; n < keys.Length; n++) KeyDown(keys[n]);
            Thread.Sleep(40);
            for (int n = keys.Length - 1; n >= 0; n--) KeyUp(keys[n]);
            Thread.Sleep(90);
        }

        public static bool IsDown(ushort vk) { return (GetAsyncKeyState(vk) & 0x8000) != 0; }

        [DllImport("user32.dll")] static extern short VkKeyScan(char ch);

        /// Types text. A character the current keyboard layout can produce is pressed as a real key (Shift held
        /// where needed), like a person typing, so key handlers that look at the key see it; anything else (Arabic
        /// on an English layout, say) goes in as a Unicode key event, as an IME or the touch keyboard sends it.
        public static void TypeText(string text)
        {
            foreach (char ch in text)
            {
                short scan = ch < 128 ? VkKeyScan(ch) : (short)-1;
                if (scan != -1 && ((scan >> 8) & 6) == 0)
                {
                    ushort vk = (ushort)(scan & 0xFF);
                    bool shift = ((scan >> 8) & 1) != 0;
                    if (shift) keybd(0x10, false);
                    keybd(vk, false);
                    keybd(vk, true);
                    if (shift) keybd(0x10, true);
                    Thread.Sleep(14);
                    continue;
                }
                INPUT[] i = new INPUT[2];
                for (int n = 0; n < 2; n++)
                {
                    i[n].type = INPUT_KEYBOARD;
                    i[n].U.ki.wVk = 0;
                    i[n].U.ki.wScan = ch;
                    i[n].U.ki.dwFlags = KEYEVENTF_UNICODE | (n == 1 ? KEYEVENTF_KEYUP : 0);
                }
                SendInput(2, i, Marshal.SizeOf(typeof(INPUT)));
                Thread.Sleep(14);
            }
            Thread.Sleep(60);
        }
    }

    /// A copy of the clipboard's text, file list, image and bitmap formats, put back by Restore. Must run on an
    /// STA thread. Other formats (private ones) can't be copied through the managed API and are lost.
    public class ClipboardBackup
    {
        string text;
        string[] files;
        byte[] image;
        bool wasEmpty;
        public string Summary = "";

        static T Retry<T>(Func<T> f, T fallback)
        {
            for (int n = 0; n < 15; n++)
            {
                try { return f(); }
                catch (Exception) { Thread.Sleep(80); }
            }
            return fallback;
        }

        public void Save()
        {
            IDataObject data = Retry<IDataObject>(delegate { return Clipboard.GetDataObject(); }, null);
            wasEmpty = data == null;
            if (data == null) { Summary = "empty"; return; }
            List<string> parts = new List<string>();
            if (data.GetDataPresent(DataFormats.UnicodeText))
            {
                text = Retry<string>(delegate { return Clipboard.GetText(TextDataFormat.UnicodeText); }, null);
                if (text != null) parts.Add("text(" + text.Length + ")");
            }
            if (data.GetDataPresent(DataFormats.FileDrop))
            {
                System.Collections.Specialized.StringCollection l = Retry<System.Collections.Specialized.StringCollection>(
                    delegate { return Clipboard.GetFileDropList(); }, null);
                if (l != null && l.Count > 0)
                {
                    files = new string[l.Count];
                    l.CopyTo(files, 0);
                    parts.Add("files(" + files.Length + ")");
                }
            }
            if (data.GetDataPresent(DataFormats.Bitmap))
            {
                Image img = Retry<Image>(delegate { return Clipboard.GetImage(); }, null);
                if (img != null)
                {
                    using (MemoryStream ms = new MemoryStream())
                    {
                        img.Save(ms, ImageFormat.Png);
                        image = ms.ToArray();
                    }
                    parts.Add("image");
                }
            }
            string[] formats = data.GetFormats();
            Summary = parts.Count == 0 ? "no restorable formats (" + string.Join(",", formats) + ")" : string.Join(" ", parts.ToArray());
        }

        /// True when the clipboard now holds what Save saw (its text, file list and whether it has an image).
        public bool Matches()
        {
            string nowText = Retry<string>(delegate { return Clipboard.ContainsText() ? Clipboard.GetText(TextDataFormat.UnicodeText) : null; }, null);
            if ((text == null || text.Length == 0) != (nowText == null || nowText.Length == 0)) return false;
            if (text != null && nowText != null && text != nowText) return false;
            string[] nowFiles = FilesNow();
            int had = files == null ? 0 : files.Length;
            if (had != nowFiles.Length) return false;
            for (int n = 0; n < had; n++) if (!string.Equals(files[n], nowFiles[n], StringComparison.OrdinalIgnoreCase)) return false;
            bool nowImage = Retry<bool>(delegate { return Clipboard.ContainsImage(); }, false);
            return (image != null) == nowImage;
        }

        public void Restore()
        {
            Retry<bool>(delegate
            {
                DataObject d = new DataObject();
                bool any = false;
                if (files != null)
                {
                    System.Collections.Specialized.StringCollection l = new System.Collections.Specialized.StringCollection();
                    l.AddRange(files);
                    d.SetFileDropList(l);
                    any = true;
                }
                if (text != null && text.Length > 0) { d.SetText(text, TextDataFormat.UnicodeText); any = true; }
                if (image != null)
                {
                    using (MemoryStream ms = new MemoryStream(image))
                    {
                        d.SetImage(new Bitmap(ms));
                    }
                    any = true;
                }
                if (any) Clipboard.SetDataObject(d, true);
                else Clipboard.Clear();
                return true;
            }, false);
        }

        /// What is on the clipboard now: file names, then text, for the checks that read it.
        public static string[] FilesNow()
        {
            return Retry<string[]>(delegate
            {
                if (!Clipboard.ContainsFileDropList()) return new string[0];
                System.Collections.Specialized.StringCollection l = Clipboard.GetFileDropList();
                string[] a = new string[l.Count];
                l.CopyTo(a, 0);
                return a;
            }, new string[0]);
        }

        public static string TextNow()
        {
            return Retry<string>(delegate { return Clipboard.ContainsText() ? Clipboard.GetText() : ""; }, "");
        }

        public static void SetTextNow(string s)
        {
            Retry<bool>(delegate { Clipboard.SetDataObject(s, true); return true; }, false);
        }
    }
}
