# Types text into THIS console's own input buffer after a pause, as key events (a test stand-in for a person).
# It touches no other window: WriteConsoleInput goes to the console this process is attached to.
param([int]$DelayMs = 1500, [string]$Text = "")
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class ConIn {
    [StructLayout(LayoutKind.Explicit, CharSet = CharSet.Unicode)]
    public struct INPUT_RECORD {
        [FieldOffset(0)] public ushort EventType;
        [FieldOffset(4)] public int bKeyDown;
        [FieldOffset(8)] public ushort wRepeatCount;
        [FieldOffset(10)] public ushort wVirtualKeyCode;
        [FieldOffset(12)] public ushort wVirtualScanCode;
        [FieldOffset(14)] public char UnicodeChar;
        [FieldOffset(16)] public uint dwControlKeyState;
    }
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern IntPtr CreateFileW(string name, uint access, uint share, IntPtr sa, uint disposition, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool WriteConsoleInputW(IntPtr handle, INPUT_RECORD[] buffer, uint count, out uint written);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetNumberOfConsoleInputEvents(IntPtr handle, out uint count);
    static IntPtr Open() { return CreateFileW("CONIN$", 0xC0000000, 3, IntPtr.Zero, 3, 0, IntPtr.Zero); }
    public static uint Pending() { uint n; GetNumberOfConsoleInputEvents(Open(), out n); return n; }
    public static uint Type(string text) {
        IntPtr handle = Open();
        uint total = 0;
        foreach (char c in text) {
            var down = new INPUT_RECORD { EventType = 1, bKeyDown = 1, wRepeatCount = 1, wVirtualKeyCode = (ushort)(c == '\r' ? 0x0D : 0), UnicodeChar = c };
            var up = down;
            up.bKeyDown = 0;
            uint written;
            WriteConsoleInputW(handle, new[] { down, up }, 2, out written);
            total += written;
        }
        return total;
    }
}
'@
Start-Sleep -Milliseconds $DelayMs
[void][ConIn]::Type($Text + "`r")
