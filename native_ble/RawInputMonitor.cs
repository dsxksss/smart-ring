using System.Collections.Concurrent;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;

internal readonly record struct RingRawMouseEvent(
    ushort ButtonFlags,
    short ButtonData,
    int DeltaX,
    int DeltaY,
    string DevicePath,
    bool IsRing);

internal sealed class RawInputMonitor : IDisposable
{
    private const uint WmInput = 0x00FF;
    private const uint WmClose = 0x0010;
    private const uint RidInput = 0x10000003;
    private const uint RidiPreparsedData = 0x20000005;
    private const uint RidiDeviceName = 0x20000007;
    private const uint RidevInputSink = 0x00000100;
    private const uint RawTypeMouse = 0;
    private const uint RawTypeHid = 2;
    private const uint ShareRead = 0x00000001;
    private const uint ShareWrite = 0x00000002;
    private const uint OpenExisting = 3;
    private const string RingAddress = "313145379C07";
    private readonly Action<RingRawMouseEvent> _mouseCallback;
    private readonly Action<byte[], string> _hidCallback;
    private readonly ConcurrentDictionary<IntPtr, string> _deviceNames = new();
    private readonly ManualResetEventSlim _started = new(false);
    private readonly Thread _thread;
    private WindowProcedure? _windowProcedure;
    private IntPtr _window;
    private Exception? _startupError;

    public RawInputMonitor(
        Action<RingRawMouseEvent> mouseCallback,
        Action<byte[], string> hidCallback)
    {
        _mouseCallback = mouseCallback;
        _hidCallback = hidCallback;
        _thread = new Thread(MessageLoop)
        {
            IsBackground = true,
            Name = "r08-raw-input",
        };
        _thread.Start();
        _started.Wait(TimeSpan.FromSeconds(3));
        if (_startupError is not null)
        {
            throw new InvalidOperationException("无法启动 R08 HID 原始输入监听", _startupError);
        }
    }

    private void MessageLoop()
    {
        try
        {
            _windowProcedure = WindowProc;
            string className = $"R08RawInput_{Environment.ProcessId}";
            var windowClass = new WindowClass
            {
                ClassName = className,
                WindowProcedure = _windowProcedure,
                Instance = GetModuleHandleW(null),
            };
            if (RegisterClassW(ref windowClass) == 0)
            {
                Marshal.ThrowExceptionForHR(Marshal.GetHRForLastWin32Error());
            }
            _window = CreateWindowExW(
                0,
                className,
                "R08 Raw Input",
                0,
                0,
                0,
                0,
                0,
                new IntPtr(-3),
                IntPtr.Zero,
                windowClass.Instance,
                IntPtr.Zero);
            if (_window == IntPtr.Zero)
            {
                Marshal.ThrowExceptionForHR(Marshal.GetHRForLastWin32Error());
            }

            RawInputDevice[] devices =
            [
                new() { UsagePage = 0x01, Usage = 0x02, Flags = RidevInputSink, Target = _window },
                new() { UsagePage = 0x0C, Usage = 0x01, Flags = RidevInputSink, Target = _window },
            ];
            if (!RegisterRawInputDevices(
                    devices,
                    (uint)devices.Length,
                    (uint)Marshal.SizeOf<RawInputDevice>()))
            {
                Marshal.ThrowExceptionForHR(Marshal.GetHRForLastWin32Error());
            }
            PrintRingDevices();
            Console.WriteLine("HID_READY 已监听 R08 鼠标与用户控制原始输入");
        }
        catch (Exception exception)
        {
            _startupError = exception;
            _started.Set();
            return;
        }

        _started.Set();
        while (GetMessageW(out Message message, IntPtr.Zero, 0, 0) > 0)
        {
            TranslateMessage(ref message);
            DispatchMessageW(ref message);
        }
    }

    private IntPtr WindowProc(IntPtr window, uint message, UIntPtr wParam, IntPtr lParam)
    {
        if (message == WmInput)
        {
            ReadRawInput(lParam);
            return IntPtr.Zero;
        }
        if (message == WmClose)
        {
            DestroyWindow(window);
            return IntPtr.Zero;
        }
        return DefWindowProcW(window, message, wParam, lParam);
    }

    private void ReadRawInput(IntPtr rawInputHandle)
    {
        uint size = 0;
        uint headerSize = (uint)Marshal.SizeOf<RawInputHeader>();
        if (GetRawInputData(rawInputHandle, RidInput, IntPtr.Zero, ref size, headerSize) != 0 || size == 0)
        {
            return;
        }
        IntPtr buffer = Marshal.AllocHGlobal((int)size);
        try
        {
            if (GetRawInputData(rawInputHandle, RidInput, buffer, ref size, headerSize) != size)
            {
                return;
            }
            RawInputHeader header = Marshal.PtrToStructure<RawInputHeader>(buffer);
            string devicePath = _deviceNames.GetOrAdd(header.Device, GetDeviceName);
            bool isRing = devicePath.Contains(RingAddress, StringComparison.OrdinalIgnoreCase);
            IntPtr data = IntPtr.Add(buffer, Marshal.SizeOf<RawInputHeader>());
            if (header.Type == RawTypeMouse)
            {
                var mouseEvent = new RingRawMouseEvent(
                    unchecked((ushort)Marshal.ReadInt16(data, 4)),
                    Marshal.ReadInt16(data, 6),
                    Marshal.ReadInt32(data, 12),
                    Marshal.ReadInt32(data, 16),
                    devicePath,
                    isRing);
                _mouseCallback(mouseEvent);
            }
            else if (header.Type == RawTypeHid && isRing)
            {
                int reportSize = Marshal.ReadInt32(data, 0);
                int reportCount = Marshal.ReadInt32(data, 4);
                int byteCount = checked(reportSize * reportCount);
                if (byteCount is > 0 and <= 4096)
                {
                    byte[] report = new byte[byteCount];
                    Marshal.Copy(IntPtr.Add(data, 8), report, 0, byteCount);
                    _hidCallback(report, devicePath);
                }
            }
        }
        finally
        {
            Marshal.FreeHGlobal(buffer);
        }
    }

    private static string GetDeviceName(IntPtr device)
    {
        uint characterCount = 0;
        GetRawInputDeviceInfoW(device, RidiDeviceName, IntPtr.Zero, ref characterCount);
        if (characterCount == 0)
        {
            return string.Empty;
        }
        var name = new StringBuilder((int)characterCount + 1);
        return GetRawInputDeviceInfoW(device, RidiDeviceName, name, ref characterCount) == uint.MaxValue
            ? string.Empty
            : name.ToString();
    }

    private static void PrintRingDevices()
    {
        uint count = 0;
        uint structureSize = (uint)Marshal.SizeOf<RawInputDeviceList>();
        GetRawInputDeviceList(null, ref count, structureSize);
        if (count == 0)
        {
            Console.WriteLine("HID_DEVICE 未枚举到原始输入设备");
            return;
        }
        var devices = new RawInputDeviceList[count];
        uint actual = GetRawInputDeviceList(devices, ref count, structureSize);
        int matched = 0;
        for (int index = 0; index < actual; index++)
        {
            string name = GetDeviceName(devices[index].Device);
            if (!name.Contains(RingAddress, StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }
            matched++;
            Console.WriteLine($"HID_DEVICE type={devices[index].Type} {name}");
            PrintHidCapabilities(devices[index].Device, name);
        }
        if (matched == 0)
        {
            Console.WriteLine("HID_DEVICE Windows 已安装 R08 HID，但当前原始输入列表未匹配到其地址");
        }
    }

    private static void PrintHidCapabilities(IntPtr device, string devicePath)
    {
        uint byteCount = 0;
        GetRawInputDeviceInfoW(device, RidiPreparsedData, IntPtr.Zero, ref byteCount);
        if (byteCount == 0)
        {
            PrintHidCapabilitiesFromPath(devicePath);
            return;
        }

        IntPtr preparsedData = Marshal.AllocHGlobal(checked((int)byteCount));
        try
        {
            uint actualByteCount = byteCount;
            uint result = GetRawInputDeviceInfoW(
                device,
                RidiPreparsedData,
                preparsedData,
                ref actualByteCount);
            if (result == uint.MaxValue || HidP_GetCaps(preparsedData, out HidpCaps caps) < 0)
            {
                Console.WriteLine("HID_CAPS 解析报告描述符失败");
                return;
            }

            Console.WriteLine(
                $"HID_CAPS usagePage=0x{caps.UsagePage:X4} usage=0x{caps.Usage:X4} " +
                $"inputBytes={caps.InputReportByteLength} outputBytes={caps.OutputReportByteLength} " +
                $"featureBytes={caps.FeatureReportByteLength} inputButtons={caps.NumberInputButtonCaps} " +
                $"inputValues={caps.NumberInputValueCaps} featureValues={caps.NumberFeatureValueCaps}");
            PrintValueCapabilities(preparsedData, 0, caps.NumberInputValueCaps, "INPUT");
            PrintValueCapabilities(preparsedData, 2, caps.NumberFeatureValueCaps, "FEATURE");
        }
        finally
        {
            Marshal.FreeHGlobal(preparsedData);
        }
    }

    private static void PrintHidCapabilitiesFromPath(string devicePath)
    {
        using SafeFileHandle handle = CreateFileW(
            devicePath,
            0,
            ShareRead | ShareWrite,
            IntPtr.Zero,
            OpenExisting,
            0,
            IntPtr.Zero);
        if (handle.IsInvalid)
        {
            Console.WriteLine($"HID_CAPS 无法打开 HID 集合 error={Marshal.GetLastWin32Error()}");
            return;
        }
        if (!HidD_GetPreparsedData(handle, out IntPtr preparsedData))
        {
            Console.WriteLine($"HID_CAPS HID 驱动未返回报告描述符 error={Marshal.GetLastWin32Error()}");
            return;
        }
        try
        {
            if (HidP_GetCaps(preparsedData, out HidpCaps caps) < 0)
            {
                Console.WriteLine("HID_CAPS HID 驱动报告描述符解析失败");
                return;
            }
            Console.WriteLine(
                $"HID_CAPS usagePage=0x{caps.UsagePage:X4} usage=0x{caps.Usage:X4} " +
                $"inputBytes={caps.InputReportByteLength} outputBytes={caps.OutputReportByteLength} " +
                $"featureBytes={caps.FeatureReportByteLength} inputButtons={caps.NumberInputButtonCaps} " +
                $"inputValues={caps.NumberInputValueCaps} featureValues={caps.NumberFeatureValueCaps}");
            PrintValueCapabilities(preparsedData, 0, caps.NumberInputValueCaps, "INPUT");
            PrintValueCapabilities(preparsedData, 2, caps.NumberFeatureValueCaps, "FEATURE");
        }
        finally
        {
            HidD_FreePreparsedData(preparsedData);
        }
    }

    private static void PrintValueCapabilities(
        IntPtr preparsedData,
        int reportType,
        ushort requestedCount,
        string label)
    {
        if (requestedCount == 0)
        {
            return;
        }

        int structureSize = Marshal.SizeOf<HidpValueCaps>();
        IntPtr valueBuffer = Marshal.AllocHGlobal(checked(structureSize * requestedCount));
        try
        {
            ushort actualCount = requestedCount;
            int status = HidP_GetValueCaps(reportType, valueBuffer, ref actualCount, preparsedData);
            if (status < 0)
            {
                Console.WriteLine($"HID_{label}_VALUE_CAPS 解析失败 status=0x{status:X8}");
                return;
            }

            for (int index = 0; index < actualCount; index++)
            {
                var valueCaps = Marshal.PtrToStructure<HidpValueCaps>(
                    IntPtr.Add(valueBuffer, index * structureSize));
                ushort usageMinimum = valueCaps.IsRange != 0
                    ? valueCaps.Range.Range.UsageMin
                    : valueCaps.Range.NotRange.Usage;
                ushort usageMaximum = valueCaps.IsRange != 0
                    ? valueCaps.Range.Range.UsageMax
                    : usageMinimum;
                Console.WriteLine(
                    $"HID_{label}_VALUE[{index}] reportId={valueCaps.ReportId} " +
                    $"page=0x{valueCaps.UsagePage:X4} usage={FormatUsageRange(valueCaps.UsagePage, usageMinimum, usageMaximum)} " +
                    $"bits={valueCaps.BitSize} count={valueCaps.ReportCount} absolute={valueCaps.IsAbsolute != 0} " +
                    $"logical={valueCaps.LogicalMin}..{valueCaps.LogicalMax} physical={valueCaps.PhysicalMin}..{valueCaps.PhysicalMax}");
            }
        }
        finally
        {
            Marshal.FreeHGlobal(valueBuffer);
        }
    }

    private static string FormatUsageRange(ushort page, ushort minimum, ushort maximum)
    {
        string FormatOne(ushort usage)
        {
            string? name = (page, usage) switch
            {
                (0x01, 0x30) => "X",
                (0x01, 0x31) => "Y",
                (0x01, 0x32) => "Z",
                (0x01, 0x33) => "Rx",
                (0x01, 0x34) => "Ry",
                (0x01, 0x35) => "Rz",
                (0x01, 0x38) => "Wheel",
                (0x0D, 0x30) => "TipPressure",
                (0x0D, 0x42) => "TipSwitch",
                (0x0D, 0x51) => "ContactIdentifier",
                (0x0D, 0x54) => "ContactCount",
                _ => null,
            };
            return name is null ? $"0x{usage:X4}" : $"0x{usage:X4}({name})";
        }

        return minimum == maximum
            ? FormatOne(minimum)
            : $"{FormatOne(minimum)}..{FormatOne(maximum)}";
    }

    public void Dispose()
    {
        if (_window != IntPtr.Zero)
        {
            PostMessageW(_window, WmClose, UIntPtr.Zero, IntPtr.Zero);
            _thread.Join(1500);
            _window = IntPtr.Zero;
        }
        _started.Dispose();
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct WindowClass
    {
        public uint Style;
        public WindowProcedure WindowProcedure;
        public int ClassExtra;
        public int WindowExtra;
        public IntPtr Instance;
        public IntPtr Icon;
        public IntPtr Cursor;
        public IntPtr Background;
        public string? MenuName;
        public string ClassName;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct RawInputDevice
    {
        public ushort UsagePage;
        public ushort Usage;
        public uint Flags;
        public IntPtr Target;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct RawInputHeader
    {
        public uint Type;
        public uint Size;
        public IntPtr Device;
        public IntPtr WParam;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct RawInputDeviceList
    {
        public IntPtr Device;
        public uint Type;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct HidpCaps
    {
        public ushort Usage;
        public ushort UsagePage;
        public ushort InputReportByteLength;
        public ushort OutputReportByteLength;
        public ushort FeatureReportByteLength;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 17)]
        public ushort[] Reserved;
        public ushort NumberLinkCollectionNodes;
        public ushort NumberInputButtonCaps;
        public ushort NumberInputValueCaps;
        public ushort NumberInputDataIndices;
        public ushort NumberOutputButtonCaps;
        public ushort NumberOutputValueCaps;
        public ushort NumberOutputDataIndices;
        public ushort NumberFeatureButtonCaps;
        public ushort NumberFeatureValueCaps;
        public ushort NumberFeatureDataIndices;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct HidpValueCaps
    {
        public ushort UsagePage;
        public byte ReportId;
        public byte IsAlias;
        public ushort BitField;
        public ushort LinkCollection;
        public ushort LinkUsage;
        public ushort LinkUsagePage;
        public byte IsRange;
        public byte IsStringRange;
        public byte IsDesignatorRange;
        public byte IsAbsolute;
        public byte HasNull;
        public byte Reserved;
        public ushort BitSize;
        public ushort ReportCount;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 5)]
        public ushort[] Reserved2;
        public uint UnitsExponent;
        public uint Units;
        public int LogicalMin;
        public int LogicalMax;
        public int PhysicalMin;
        public int PhysicalMax;
        public HidpValueCapsUnion Range;
    }

    [StructLayout(LayoutKind.Explicit)]
    private struct HidpValueCapsUnion
    {
        [FieldOffset(0)]
        public HidpValueCapsRange Range;
        [FieldOffset(0)]
        public HidpValueCapsNotRange NotRange;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct HidpValueCapsRange
    {
        public ushort UsageMin;
        public ushort UsageMax;
        public ushort StringMin;
        public ushort StringMax;
        public ushort DesignatorMin;
        public ushort DesignatorMax;
        public ushort DataIndexMin;
        public ushort DataIndexMax;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct HidpValueCapsNotRange
    {
        public ushort Usage;
        public ushort Reserved1;
        public ushort StringIndex;
        public ushort Reserved2;
        public ushort DesignatorIndex;
        public ushort Reserved3;
        public ushort DataIndex;
        public ushort Reserved4;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct Message
    {
        public IntPtr Window;
        public uint MessageId;
        public UIntPtr WParam;
        public IntPtr LParam;
        public uint Time;
        public int PointX;
        public int PointY;
        public uint Private;
    }

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate IntPtr WindowProcedure(IntPtr window, uint message, UIntPtr wParam, IntPtr lParam);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr GetModuleHandleW(string? moduleName);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern ushort RegisterClassW(ref WindowClass windowClass);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CreateWindowExW(
        uint extendedStyle,
        string className,
        string windowName,
        uint style,
        int x,
        int y,
        int width,
        int height,
        IntPtr parent,
        IntPtr menu,
        IntPtr instance,
        IntPtr parameter);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool RegisterRawInputDevices(
        RawInputDevice[] devices,
        uint deviceCount,
        uint structureSize);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern uint GetRawInputDeviceList(
        [Out] RawInputDeviceList[]? devices,
        ref uint deviceCount,
        uint structureSize);

    [DllImport("user32.dll")]
    private static extern uint GetRawInputData(
        IntPtr rawInput,
        uint command,
        IntPtr data,
        ref uint size,
        uint headerSize);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern uint GetRawInputDeviceInfoW(
        IntPtr device,
        uint command,
        IntPtr data,
        ref uint size);

    [DllImport("hid.dll")]
    private static extern int HidP_GetCaps(IntPtr preparsedData, out HidpCaps capabilities);

    [DllImport("hid.dll", SetLastError = true)]
    private static extern bool HidD_GetPreparsedData(
        SafeFileHandle hidDeviceObject,
        out IntPtr preparsedData);

    [DllImport("hid.dll")]
    private static extern bool HidD_FreePreparsedData(IntPtr preparsedData);

    [DllImport("hid.dll")]
    private static extern int HidP_GetValueCaps(
        int reportType,
        IntPtr valueCapabilities,
        ref ushort valueCapabilitiesLength,
        IntPtr preparsedData);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFileW(
        string fileName,
        uint desiredAccess,
        uint shareMode,
        IntPtr securityAttributes,
        uint creationDisposition,
        uint flagsAndAttributes,
        IntPtr templateFile);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern uint GetRawInputDeviceInfoW(
        IntPtr device,
        uint command,
        StringBuilder data,
        ref uint size);

    [DllImport("user32.dll")]
    private static extern int GetMessageW(out Message message, IntPtr window, uint minimum, uint maximum);

    [DllImport("user32.dll")]
    private static extern bool TranslateMessage(ref Message message);

    [DllImport("user32.dll")]
    private static extern IntPtr DispatchMessageW(ref Message message);

    [DllImport("user32.dll")]
    private static extern IntPtr DefWindowProcW(IntPtr window, uint message, UIntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll")]
    private static extern bool PostMessageW(IntPtr window, uint message, UIntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll")]
    private static extern bool DestroyWindow(IntPtr window);
}
