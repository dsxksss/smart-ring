using System.ComponentModel;
using System.Collections.Concurrent;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;

internal static class Program
{
    private const string DefaultServicePath = @"\\?\BTHLEDevice#{6e40fff0-b5a3-f393-e0a9-e50e24dcca9e}_313145379c07#9&25ace2eb&1&000e#{6e3bb679-4372-40c8-9eaa-4509df260cd8}";
    private const string DefaultDeviceInfoServicePath = @"\\?\BTHLEDevice#{0000180a-0000-1000-8000-00805f9b34fb}_313145379c07#9&25ace2eb&1&001a#{6e3bb679-4372-40c8-9eaa-4509df260cd8}";
    private const uint GenericRead = 0x80000000;
    private const uint GenericWrite = 0x40000000;
    private const uint ShareRead = 0x00000001;
    private const uint ShareWrite = 0x00000002;
    private const uint OpenExisting = 3;
    private static readonly Guid WriteUuid = new("6e400002-b5a3-f393-e0a9-e50e24dcca9e");
    private static readonly Guid NotifyUuid = new("6e400003-b5a3-f393-e0a9-e50e24dcca9e");
    private static readonly byte[] TestOpen = [0xC9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xC9];
    private static readonly byte[] TestClose = [0xCA, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xCA];
    private static GattEventCallback? _callback;
    private static WindowsRingController? _controller;

    public static int Main(string[] args)
    {
        Console.OutputEncoding = Encoding.UTF8;
        TextWriter originalOut = Console.Out;
        TextWriter originalError = Console.Error;
        StreamWriter? logWriter = null;
        bool loggingEnabled = args.Contains("--control", StringComparer.OrdinalIgnoreCase);
        try
        {
            if (loggingEnabled)
            {
                string logPath = Path.Combine(Environment.CurrentDirectory, "r08-control-latest.log");
                logWriter = new StreamWriter(
                    new FileStream(logPath, FileMode.Create, FileAccess.Write, FileShare.ReadWrite),
                    new UTF8Encoding(false))
                {
                    AutoFlush = true,
                };
                object writeLock = new();
                Console.SetOut(new TeeTextWriter(originalOut, logWriter, writeLock));
                Console.SetError(new TeeTextWriter(originalError, logWriter, writeLock));
                Console.WriteLine($"LOG_FILE {logPath}");
            }
            return Run(args);
        }
        catch (Exception exception)
        {
            Console.Error.WriteLine("FATAL 程序异常退出：");
            Console.Error.WriteLine(exception);
            return 1;
        }
        finally
        {
            Console.Out.Flush();
            Console.Error.Flush();
            Console.SetOut(originalOut);
            Console.SetError(originalError);
            logWriter?.Dispose();
        }
    }

    private static int Run(string[] args)
    {
        if (args.Contains("--device-info", StringComparer.OrdinalIgnoreCase))
        {
            return ReadDeviceInformation();
        }

        bool runTest = args.Contains("--test", StringComparer.OrdinalIgnoreCase);
        bool listenOnly = args.Contains("--listen", StringComparer.OrdinalIgnoreCase);
        bool controlWindows = args.Contains("--control", StringComparer.OrdinalIgnoreCase);
        int seconds = 25;
        int secondsIndex = Array.FindIndex(args, value => value.Equals("--seconds", StringComparison.OrdinalIgnoreCase));
        if (secondsIndex >= 0 && secondsIndex + 1 < args.Length)
        {
            seconds = Math.Clamp(int.Parse(args[secondsIndex + 1]), 1, 300);
        }
        else if (controlWindows)
        {
            seconds = 0;
        }
        int touchType = GetIntegerOption(args, "--touch-type", 2, 0, 10);
        int sleepMinutes = GetIntegerOption(args, "--sleep-minutes", 5, 1, 10);
        int scrollGain = GetIntegerOption(args, "--scroll-gain", 4, 1, 10);
        string path = DefaultServicePath;
        using SafeFileHandle handle = CreateFileW(
            path,
            GenericRead | GenericWrite,
            ShareRead | ShareWrite,
            IntPtr.Zero,
            OpenExisting,
            0,
            IntPtr.Zero);
        if (handle.IsInvalid)
        {
            throw new Win32Exception(Marshal.GetLastWin32Error(), $"无法打开 GATT 服务接口：{path}");
        }

        ushort required;
        int first = BluetoothGATTGetCharacteristics(
            handle, IntPtr.Zero, 0, null, out required, 0);
        Console.WriteLine($"服务句柄已打开；查询特征 HRESULT=0x{first:X8}，数量={required}");
        if (required == 0)
        {
            return 2;
        }

        var characteristics = new BthLeGattCharacteristic[required];
        int result = BluetoothGATTGetCharacteristics(
            handle, IntPtr.Zero, required, characteristics, out ushort actual, 0);
        Console.WriteLine($"枚举特征 HRESULT=0x{result:X8}，实际数量={actual}");
        if (result < 0)
        {
            return 3;
        }

        BthLeGattCharacteristic? writeCharacteristic = null;
        BthLeGattCharacteristic? notifyCharacteristic = null;
        BthLeGattDescriptor? cccd = null;
        for (int index = 0; index < actual; index++)
        {
            BthLeGattCharacteristic item = characteristics[index];
            Console.WriteLine(
                $"[{index}] {FormatUuid(item.CharacteristicUuid)} " +
                $"attr=0x{item.AttributeHandle:X4} value=0x{item.CharacteristicValueHandle:X4} " +
                $"read={item.IsReadable != 0} write={item.IsWritable != 0} " +
                $"writeNR={item.IsWritableWithoutResponse != 0} notify={item.IsNotifiable != 0}");
            if (item.IsNotifiable != 0)
            {
                cccd = PrintDescriptors(handle, ref item);
            }
            Guid itemUuid = ExpandUuid(item.CharacteristicUuid);
            if (itemUuid == WriteUuid)
            {
                writeCharacteristic = item;
            }
            if (itemUuid == NotifyUuid)
            {
                notifyCharacteristic = item;
            }
        }
        if (runTest || listenOnly || controlWindows)
        {
            if (writeCharacteristic is null || notifyCharacteristic is null || cccd is null)
            {
                throw new InvalidOperationException("缺少写入特征、通知特征或 0x2902 描述符");
            }
            RunNotificationSession(
                handle,
                writeCharacteristic.Value,
                notifyCharacteristic.Value,
                cccd.Value,
                seconds,
                runTest,
                controlWindows,
                touchType,
                sleepMinutes,
                scrollGain);
        }
        return 0;
    }

    private static int ReadDeviceInformation()
    {
        using SafeFileHandle handle = CreateFileW(
            DefaultDeviceInfoServicePath,
            GenericRead,
            ShareRead | ShareWrite,
            IntPtr.Zero,
            OpenExisting,
            0,
            IntPtr.Zero);
        if (handle.IsInvalid)
        {
            throw new Win32Exception(
                Marshal.GetLastWin32Error(),
                $"无法以只读方式打开设备信息服务：{DefaultDeviceInfoServicePath}");
        }

        int first = BluetoothGATTGetCharacteristics(
            handle, IntPtr.Zero, 0, null, out ushort required, 0);
        Console.WriteLine($"DEVICE_INFO_READ_ONLY 查询特征 HRESULT=0x{first:X8}，数量={required}");
        if (required == 0)
        {
            return 2;
        }

        var characteristics = new BthLeGattCharacteristic[required];
        int result = BluetoothGATTGetCharacteristics(
            handle, IntPtr.Zero, required, characteristics, out ushort actual, 0);
        Console.WriteLine($"DEVICE_INFO_READ_ONLY 枚举特征 HRESULT=0x{result:X8}，实际数量={actual}");
        if (result < 0)
        {
            return 3;
        }

        for (int index = 0; index < actual; index++)
        {
            BthLeGattCharacteristic characteristic = characteristics[index];
            Guid uuid = ExpandUuid(characteristic.CharacteristicUuid);
            string label = uuid switch
            {
                var value when value == ExpandShortUuid(0x2A23) => "System ID",
                var value when value == ExpandShortUuid(0x2A24) => "Model Number",
                var value when value == ExpandShortUuid(0x2A25) => "Serial Number",
                var value when value == ExpandShortUuid(0x2A26) => "Firmware Revision",
                var value when value == ExpandShortUuid(0x2A27) => "Hardware Revision",
                var value when value == ExpandShortUuid(0x2A28) => "Software Revision",
                var value when value == ExpandShortUuid(0x2A29) => "Manufacturer Name",
                var value when value == ExpandShortUuid(0x2A50) => "PnP ID",
                _ => uuid.ToString(),
            };
            if (characteristic.IsReadable == 0)
            {
                Console.WriteLine($"{label}: <不可读>");
                continue;
            }

            byte[] valueBytes = ReadCharacteristicValue(handle, ref characteristic);
            string textValue = Encoding.UTF8.GetString(valueBytes).TrimEnd('\0');
            bool printable = textValue.Length > 0 && textValue.All(character => !char.IsControl(character));
            Console.WriteLine(
                $"{label}: {(printable ? textValue : "<二进制>")}  [HEX {FormatBytes(valueBytes)}]");
        }
        return 0;
    }

    private static byte[] ReadCharacteristicValue(
        SafeFileHandle handle,
        ref BthLeGattCharacteristic characteristic)
    {
        const uint ForceReadFromDevice = 0x00000002;
        int first = BluetoothGATTGetCharacteristicValue(
            handle,
            ref characteristic,
            0,
            IntPtr.Zero,
            out ushort required,
            ForceReadFromDevice);
        if (required < sizeof(uint))
        {
            throw new InvalidOperationException(
                $"读取 {FormatUuid(characteristic.CharacteristicUuid)} 长度失败：HRESULT=0x{first:X8}，required={required}");
        }

        IntPtr value = Marshal.AllocHGlobal(required);
        try
        {
            int result = BluetoothGATTGetCharacteristicValue(
                handle,
                ref characteristic,
                required,
                value,
                out ushort actual,
                ForceReadFromDevice);
            RequireSuccess(result, $"只读读取 {FormatUuid(characteristic.CharacteristicUuid)}");
            int dataSize = Marshal.ReadInt32(value, 0);
            if (dataSize < 0 || dataSize > actual - sizeof(uint))
            {
                throw new InvalidOperationException($"设备返回了无效的特征长度：{dataSize}，缓冲区={actual}");
            }
            byte[] data = new byte[dataSize];
            Marshal.Copy(IntPtr.Add(value, sizeof(uint)), data, 0, dataSize);
            return data;
        }
        finally
        {
            Marshal.FreeHGlobal(value);
        }
    }

    private static Guid ExpandShortUuid(ushort value) =>
        new($"0000{value:X4}-0000-1000-8000-00805f9b34fb");

    private static int GetIntegerOption(
        string[] args,
        string name,
        int defaultValue,
        int minimum,
        int maximum)
    {
        int index = Array.FindIndex(args, value => value.Equals(name, StringComparison.OrdinalIgnoreCase));
        if (index < 0 || index + 1 >= args.Length)
        {
            return defaultValue;
        }
        if (!int.TryParse(args[index + 1], out int value))
        {
            throw new ArgumentException($"{name} 后面必须是整数");
        }
        return Math.Clamp(value, minimum, maximum);
    }

    private static BthLeGattDescriptor? PrintDescriptors(
        SafeFileHandle handle,
        ref BthLeGattCharacteristic characteristic)
    {
        int first = BluetoothGATTGetDescriptors(
            handle, ref characteristic, 0, null, out ushort required, 0);
        Console.WriteLine($"    查询描述符 HRESULT=0x{first:X8}，数量={required}");
        if (required == 0)
        {
            return null;
        }
        var descriptors = new BthLeGattDescriptor[required];
        int result = BluetoothGATTGetDescriptors(
            handle, ref characteristic, required, descriptors, out ushort actual, 0);
        Console.WriteLine($"    枚举描述符 HRESULT=0x{result:X8}，实际数量={actual}");
        for (int index = 0; index < actual; index++)
        {
            Console.WriteLine(
                $"    [{index}] type={descriptors[index].DescriptorType} " +
                $"uuid={FormatUuid(descriptors[index].DescriptorUuid)} " +
                $"attr=0x{descriptors[index].AttributeHandle:X4}");
        }
        return descriptors.Take(actual).FirstOrDefault(item => item.DescriptorType == 2);
    }

    private static void RunNotificationSession(
        SafeFileHandle handle,
        BthLeGattCharacteristic writeCharacteristic,
        BthLeGattCharacteristic notifyCharacteristic,
        BthLeGattDescriptor cccd,
        int seconds,
        bool sensorTest,
        bool controlWindows,
        int touchType,
        int sleepMinutes,
        int scrollGain)
    {
        IntPtr eventHandle = IntPtr.Zero;
        bool cccdEnabled = false;
        bool testOpened = false;
        bool controllerTouchEnabled = false;
        ConsoleControlHandler? consoleCloseHandler = null;
        try
        {
            SetNotifications(handle, ref cccd, true);
            cccdEnabled = true;

            var registration = new GattValueChangedRegistration
            {
                NumCharacteristics = 1,
                Characteristic = notifyCharacteristic,
            };
            _callback = OnGattEvent;
            int registerResult = BluetoothGATTRegisterEvent(
                handle,
                0,
                ref registration,
                _callback,
                IntPtr.Zero,
                out eventHandle,
                0);
            RequireSuccess(registerResult, "注册通知回调");
            Console.WriteLine("通知已启用并注册回调");

            if (sensorTest)
            {
                WritePacket(handle, ref writeCharacteristic, TestOpen);
                testOpened = true;
                Console.WriteLine($"TEST_READY 请持续缓慢转动戒指；监听 {seconds} 秒");
            }
            else
            {
                WritePacket(handle, ref writeCharacteristic, BuildTouchPacket(touchType, sleepMinutes));
                if (controlWindows)
                {
                    controllerTouchEnabled = touchType != 0;
                    consoleCloseHandler = controlType =>
                    {
                        if (controlType is 2 or 5 or 6)
                        {
                            TryDisableTouch(handle, writeCharacteristic, sleepMinutes, logResult: false);
                        }
                        return false;
                    };
                    SetConsoleCtrlHandler(consoleCloseHandler, true);
                    _controller = new WindowsRingController(scrollGain);
                    Console.WriteLine(
                        $"CONTROL_READY 类型={touchType}；滚轮增益={scrollGain}；双击=复制，三击=粘贴，按 Enter 退出");
                }
                else
                {
                    Console.WriteLine(
                        $"LISTEN_READY 类型={touchType}；请操作戒指触控区；监听 {seconds} 秒");
                }
            }
            if (controlWindows)
            {
                if (seconds > 0)
                {
                    Thread.Sleep(TimeSpan.FromSeconds(seconds));
                }
                else
                {
                    ConsoleCancelEventHandler cancelHandler = (_, eventArgs) =>
                    {
                        if (eventArgs.SpecialKey == ConsoleSpecialKey.ControlC)
                        {
                            eventArgs.Cancel = true;
                            Console.WriteLine("控制程序仍在运行；需要退出请按 Enter。");
                        }
                    };
                    Console.CancelKeyPress += cancelHandler;
                    try
                    {
                        Console.WriteLine("请切换到要控制的窗口；回到这里按 Enter 可退出。");
                        Console.ReadLine();
                    }
                    finally
                    {
                        Console.CancelKeyPress -= cancelHandler;
                    }
                }
            }
            else
            {
                Thread.Sleep(TimeSpan.FromSeconds(seconds));
            }
        }
        finally
        {
            _controller?.Dispose();
            _controller = null;
            if (controllerTouchEnabled)
            {
                TryDisableTouch(handle, writeCharacteristic, sleepMinutes, logResult: true);
            }
            if (consoleCloseHandler is not null)
            {
                SetConsoleCtrlHandler(consoleCloseHandler, false);
            }
            if (testOpened)
            {
                try
                {
                    WritePacket(handle, ref writeCharacteristic, TestClose);
                    Console.WriteLine("TEST_DONE 已发送 CA 关闭测试模式");
                }
                catch (Exception exception)
                {
                    Console.Error.WriteLine($"警告：发送 CA 失败：{exception.Message}");
                }
            }
            if (eventHandle != IntPtr.Zero)
            {
                BluetoothGATTUnregisterEvent(eventHandle, 0);
            }
            if (cccdEnabled)
            {
                try
                {
                    SetNotifications(handle, ref cccd, false);
                }
                catch
                {
                }
            }
        }
    }

    private static void TryDisableTouch(
        SafeFileHandle handle,
        BthLeGattCharacteristic characteristic,
        int sleepMinutes,
        bool logResult)
    {
        try
        {
            WritePacket(handle, ref characteristic, BuildTouchPacket(0, sleepMinutes));
            if (logResult)
            {
                Thread.Sleep(150);
                Console.WriteLine("CONTROL_DONE 已关闭戒指触控模式，戒指不再作为系统鼠标输入");
            }
        }
        catch (Exception exception)
        {
            if (logResult)
            {
                Console.Error.WriteLine($"警告：关闭戒指触控模式失败：{exception.Message}");
            }
        }
    }

    private static byte[] BuildTouchPacket(int appType, int sleepMinutes)
    {
        byte[] packet = new byte[16];
        packet[0] = 0x3B;
        packet[1] = 0x02;
        packet[2] = 0x00;
        packet[3] = (byte)appType;
        packet[4] = (byte)sleepMinutes;
        packet[15] = unchecked((byte)packet.Take(15).Sum(value => value));
        return packet;
    }

    private static void OnGattEvent(int eventType, IntPtr eventOutParameter, IntPtr context)
    {
        if (eventType != 0 || eventOutParameter == IntPtr.Zero)
        {
            return;
        }
        ushort changedHandle = unchecked((ushort)Marshal.ReadInt16(eventOutParameter, 0));
        IntPtr valuePointer = Marshal.ReadIntPtr(eventOutParameter, 16);
        if (valuePointer == IntPtr.Zero)
        {
            return;
        }
        int dataSize = Marshal.ReadInt32(valuePointer, 0);
        if (dataSize < 0 || dataSize > 4096)
        {
            return;
        }
        byte[] data = new byte[dataSize];
        Marshal.Copy(IntPtr.Add(valuePointer, 4), data, 0, dataSize);
        Console.WriteLine(
            $"{DateTimeOffset.Now:O} RX handle=0x{changedHandle:X4}  {FormatBytes(data)}");
        _controller?.HandlePacket(data);
    }

    private static void SetNotifications(
        SafeFileHandle handle,
        ref BthLeGattDescriptor descriptor,
        bool enabled)
    {
        var value = new BthLeGattDescriptorValue
        {
            DescriptorType = 2,
            DescriptorUuid = CreateShortUuid(0x2902),
            IsSubscribeToNotification = enabled ? (byte)1 : (byte)0,
            IsSubscribeToIndication = 0,
            DataSize = 0,
        };
        int result = BluetoothGATTSetDescriptorValue(handle, ref descriptor, ref value, 0);
        RequireSuccess(result, enabled ? "启用 0x2902 通知" : "关闭 0x2902 通知");
    }

    private static void WritePacket(
        SafeFileHandle handle,
        ref BthLeGattCharacteristic characteristic,
        byte[] data)
    {
        IntPtr value = Marshal.AllocHGlobal(4 + data.Length);
        try
        {
            Marshal.WriteInt32(value, data.Length);
            Marshal.Copy(data, 0, IntPtr.Add(value, 4), data.Length);
            int result = BluetoothGATTSetCharacteristicValue(
                handle, ref characteristic, value, 0, 0);
            RequireSuccess(result, $"写入 {FormatBytes(data)}");
            Console.WriteLine($"{DateTimeOffset.Now:O} TX  {FormatBytes(data)}");
        }
        finally
        {
            Marshal.FreeHGlobal(value);
        }
    }

    private static void RequireSuccess(int hresult, string operation)
    {
        if (hresult < 0)
        {
            Marshal.ThrowExceptionForHR(hresult);
        }
        Console.WriteLine($"{operation} HRESULT=0x{hresult:X8}");
    }

    private static string FormatBytes(byte[] data)
    {
        return string.Join(" ", data.Select(value => value.ToString("X2")));
    }

    private sealed class TeeTextWriter : TextWriter
    {
        private readonly TextWriter _console;
        private readonly TextWriter _log;
        private readonly object _writeLock;

        public TeeTextWriter(TextWriter console, TextWriter log, object writeLock)
        {
            _console = console;
            _log = log;
            _writeLock = writeLock;
        }

        public override Encoding Encoding => _console.Encoding;

        public override void Write(char value)
        {
            lock (_writeLock)
            {
                _console.Write(value);
                _log.Write(value);
            }
        }

        public override void Write(string? value)
        {
            lock (_writeLock)
            {
                _console.Write(value);
                _log.Write(value);
            }
        }

        public override void WriteLine(string? value)
        {
            lock (_writeLock)
            {
                _console.WriteLine(value);
                _log.WriteLine(value);
            }
        }

        public override void Flush()
        {
            lock (_writeLock)
            {
                _console.Flush();
                _log.Flush();
            }
        }
    }

    private static Guid ExpandUuid(BthLeUuid value)
    {
        return value.IsShortUuid != 0
            ? new Guid($"0000{value.Value.ShortUuid:x4}-0000-1000-8000-00805f9b34fb")
            : value.Value.LongUuid;
    }

    private static BthLeUuid CreateShortUuid(ushort value)
    {
        return new BthLeUuid
        {
            IsShortUuid = 1,
            Value = new BthLeUuidValue { ShortUuid = value },
        };
    }

    private static string FormatUuid(BthLeUuid value)
    {
        if (value.IsShortUuid != 0)
        {
            return $"0000{value.Value.ShortUuid:x4}-0000-1000-8000-00805f9b34fb";
        }
        return value.Value.LongUuid.ToString();
    }

    private sealed class WindowsRingController : IDisposable
    {
        private const byte VkControl = 0x11;
        private const byte VkLeft = 0x25;
        private const byte VkRight = 0x27;
        private const byte VkC = 0x43;
        private const byte VkV = 0x56;
        private const uint KeyeventfKeyup = 0x0002;
        private const uint MouseeventfLeftup = 0x0004;
        private const uint MouseeventfWheel = 0x0800;
        private const int TapFlushMilliseconds = 850;
        private readonly object _tapLock = new();
        private readonly Timer _tapTimer;
        private readonly object _scrollLock = new();
        private readonly ConcurrentQueue<int> _scrollDeltas = new();
        private readonly AutoResetEvent _scrollReady = new(false);
        private readonly Thread _scrollThread;
        private readonly RawInputMonitor _rawInput;
        private readonly object _cursorLock = new();
        private volatile bool _disposed;
        private int _tapCount;
        private int _scrollDirection;
        private int _heldScrollDirection;
        private long _heldScrollStartedTicks;
        private int _heldScrollOutputMagnitude;
        private long _lastTapTicks;
        private ScreenPoint _cursorAnchor;
        private bool _ringButtonDown;
        private bool _ringGestureMoved;
        private readonly int _scrollGain;

        public WindowsRingController(int scrollGain)
        {
            _scrollGain = scrollGain;
            _tapTimer = new Timer(_ => FlushTaps(), null, Timeout.Infinite, Timeout.Infinite);
            _scrollThread = new Thread(ScrollLoop)
            {
                IsBackground = true,
                Name = "r08-smooth-scroll",
            };
            _scrollThread.Start();
            GetCursorPos(out _cursorAnchor);
            _rawInput = new RawInputMonitor(HandleRawMouse, HandleRawHid);
        }

        public void HandlePacket(byte[] data)
        {
            if (data.Length >= 2 && data[0] == 0x02 && data[1] == 0x02)
            {
                Console.WriteLine("ACTION R08 兼容按键通知（HID 已处理，不重复计数）");
                return;
            }
            if (data.Length < 2 || data[0] != 0x1D)
            {
                return;
            }
            switch (data[1])
            {
                case 1:
                    RecordTap();
                    Console.WriteLine("ACTION 点击（等待判断双击/三击）");
                    break;
                case 2:
                    QueueScroll(-1);
                    Console.WriteLine("ACTION 下滑 -> 向下平滑滚动");
                    break;
                case 3:
                    QueueScroll(1);
                    Console.WriteLine("ACTION 上滑 -> 向上平滑滚动");
                    break;
                case 4:
                    PressKey(VkLeft);
                    Console.WriteLine("ACTION 动作4 -> 光标左移");
                    break;
                case 5:
                    PressKey(VkRight);
                    Console.WriteLine("ACTION 动作5 -> 光标右移");
                    break;
                default:
                    Console.WriteLine($"ACTION 未映射的触控动作 {data[1]}");
                    break;
            }
        }

        private void RecordTap()
        {
            long now = Environment.TickCount64;
            long previous = Interlocked.Exchange(ref _lastTapTicks, now);
            if (previous != 0 && now - previous < 60)
            {
                return;
            }
            lock (_tapLock)
            {
                _tapCount++;
                _tapTimer.Change(TapFlushMilliseconds, Timeout.Infinite);
            }
        }

        private void HandleRawMouse(RingRawMouseEvent input)
        {
            const ushort leftButtonDown = 0x0001;
            const ushort leftButtonUp = 0x0002;
            const ushort verticalWheel = 0x0400;
            const ushort horizontalWheel = 0x0800;

            if (!input.IsRing)
            {
                if (input.DeltaX != 0 || input.DeltaY != 0)
                {
                    lock (_cursorLock)
                    {
                        GetCursorPos(out _cursorAnchor);
                    }
                }
                return;
            }

            Console.WriteLine(
                $"HID_MOUSE_R08 buttons=0x{input.ButtonFlags:X4} data={input.ButtonData} " +
                $"dx={input.DeltaX} dy={input.DeltaY}");
            if ((input.ButtonFlags & leftButtonDown) != 0)
            {
                _ringButtonDown = true;
                _ringGestureMoved = false;
                ResetHeldScroll();
                mouse_event(MouseeventfLeftup, 0, 0, 0, UIntPtr.Zero);
                Console.WriteLine("ACTION R08 触控开始；已立即释放系统左键，避免拖拽/按住");
            }
            if (input.DeltaX != 0 || input.DeltaY != 0)
            {
                lock (_cursorLock)
                {
                    SetCursorPos(_cursorAnchor.X, _cursorAnchor.Y);
                }
                int absoluteX = Math.Abs(input.DeltaX);
                int absoluteY = Math.Abs(input.DeltaY);
                if (_ringButtonDown && input.DeltaX == 0 && absoluteY is >= 1 and <= 32)
                {
                    _ringGestureMoved = true;
                    mouse_event(MouseeventfLeftup, 0, 0, 0, UIntPtr.Zero);
                    int wheelDelta = -Math.Sign(input.DeltaY) * Math.Clamp(absoluteY * _scrollGain, 16, 120);
                    StartHeldScroll(Math.Sign(wheelDelta));
                    Console.WriteLine(
                        $"ACTION R08 滑动 dy={input.DeltaY} -> 精细滚动方向已识别；短划一格，保持触摸连续滚动");
                }
                else if (_ringButtonDown && input.DeltaY == 0 && absoluteX is >= 1 and <= 32)
                {
                    _ringGestureMoved = true;
                    ResetHeldScroll();
                    PressKey(input.DeltaX < 0 ? VkLeft : VkRight);
                    Console.WriteLine(
                        $"ACTION R08 横滑 dx={input.DeltaX} -> 光标{(input.DeltaX < 0 ? "左" : "右")}移；鼠标指针已恢复");
                }
                else
                {
                    Console.WriteLine("ACTION R08 前导/结束校准位移，已忽略");
                }
            }
            if ((input.ButtonFlags & leftButtonUp) != 0)
            {
                (int heldDirection, int heldOutputMagnitude) = FinishHeldScroll();
                if (_ringButtonDown && !_ringGestureMoved)
                {
                    RecordTap();
                    Console.WriteLine("ACTION R08 点击完成（等待判断双击/三击）");
                }
                else if (_ringButtonDown)
                {
                    if (heldDirection != 0 && heldOutputMagnitude < 120)
                    {
                        QueueSmoothWheel(heldDirection * (120 - heldOutputMagnitude));
                        Console.WriteLine("ACTION R08 短划完成 -> 滚动一个标准刻度");
                    }
                    else
                    {
                        Console.WriteLine("ACTION R08 持续滚动结束 -> 已立即停止，不计入点击次数");
                    }
                }
                _ringButtonDown = false;
                _ringGestureMoved = false;
            }
            if ((input.ButtonFlags & verticalWheel) != 0)
            {
                Console.WriteLine(
                    $"ACTION HID 垂直滚轮 {(input.ButtonData > 0 ? "向上" : "向下")}（由 Windows 原生处理）");
            }
            if ((input.ButtonFlags & horizontalWheel) != 0)
            {
                PressKey(input.ButtonData < 0 ? VkLeft : VkRight);
                Console.WriteLine(
                    $"ACTION HID 水平滚轮 -> 光标{(input.ButtonData < 0 ? "左" : "右")}移");
            }
        }

        private static void HandleRawHid(byte[] report, string devicePath)
        {
            Console.WriteLine($"HID_REPORT {FormatBytes(report)}");
        }

        private void FlushTaps()
        {
            int count;
            lock (_tapLock)
            {
                count = _tapCount;
                _tapCount = 0;
            }
            if (count == 2)
            {
                Hotkey(VkControl, VkC);
                Console.WriteLine("ACTION 双击 -> Ctrl+C 复制");
            }
            else if (count >= 3)
            {
                Hotkey(VkControl, VkV);
                Console.WriteLine("ACTION 三击 -> Ctrl+V 粘贴");
            }
            else if (count == 1)
            {
                Console.WriteLine("ACTION 单击 -> 无操作");
            }
        }

        private void QueueScroll(int direction)
        {
            QueueSmoothWheel(direction * 240);
        }

        private void QueueSmoothWheel(int totalDelta)
        {
            const int stepSize = 16;
            const int maximumQueuedSteps = 96;
            int direction = Math.Sign(totalDelta);
            if (direction == 0)
            {
                return;
            }

            lock (_scrollLock)
            {
                // Do not let unfinished inertia from the previous direction cancel
                // a quick reversal of the user's finger.
                if (_scrollDirection != 0 && _scrollDirection != direction)
                {
                    while (_scrollDeltas.TryDequeue(out _))
                    {
                    }
                }

                _scrollDirection = direction;
                int remaining = Math.Abs(totalDelta);
                while (remaining > 0 && _scrollDeltas.Count < maximumQueuedSteps)
                {
                    int step = Math.Min(stepSize, remaining);
                    _scrollDeltas.Enqueue(direction * step);
                    remaining -= step;
                }
            }
            _scrollReady.Set();
        }

        private void StartHeldScroll(int direction)
        {
            lock (_scrollLock)
            {
                if (_heldScrollDirection != direction)
                {
                    if (_scrollDirection != 0 && _scrollDirection != direction)
                    {
                        while (_scrollDeltas.TryDequeue(out _))
                        {
                        }
                    }
                    _scrollDirection = direction;
                    _heldScrollStartedTicks = Environment.TickCount64;
                    _heldScrollOutputMagnitude = 0;
                }
                _heldScrollDirection = direction;
            }
            _scrollReady.Set();
        }

        private void ResetHeldScroll()
        {
            lock (_scrollLock)
            {
                _heldScrollDirection = 0;
                _heldScrollStartedTicks = 0;
                _heldScrollOutputMagnitude = 0;
            }
        }

        private (int Direction, int OutputMagnitude) FinishHeldScroll()
        {
            lock (_scrollLock)
            {
                int direction = _heldScrollDirection;
                int outputMagnitude = _heldScrollOutputMagnitude;
                _heldScrollDirection = 0;
                _heldScrollStartedTicks = 0;
                _heldScrollOutputMagnitude = 0;
                return (direction, outputMagnitude);
            }
        }

        private void ScrollLoop()
        {
            while (!_disposed)
            {
                _scrollReady.WaitOne();
                while (!_disposed)
                {
                    int delta;
                    lock (_scrollLock)
                    {
                        if (!_scrollDeltas.TryDequeue(out delta))
                        {
                            if (_heldScrollDirection != 0)
                            {
                                long heldMilliseconds = Environment.TickCount64 - _heldScrollStartedTicks;
                                if (heldMilliseconds < 300)
                                {
                                    delta = 0;
                                }
                                else
                                {
                                    int heldStep = heldMilliseconds >= 3000 ? 8 : heldMilliseconds >= 1500 ? 4 : 2;
                                    delta = _heldScrollDirection * heldStep;
                                    _heldScrollOutputMagnitude += heldStep;
                                }
                            }
                            else
                            {
                                _scrollDirection = 0;
                                break;
                            }
                        }
                    }
                    if (delta != 0)
                    {
                        mouse_event(MouseeventfWheel, 0, 0, delta, UIntPtr.Zero);
                    }
                    Thread.Sleep(10);
                }
            }
        }

        private static void PressKey(byte key)
        {
            keybd_event(key, 0, 0, UIntPtr.Zero);
            keybd_event(key, 0, KeyeventfKeyup, UIntPtr.Zero);
        }

        private static void Hotkey(byte modifier, byte key)
        {
            keybd_event(modifier, 0, 0, UIntPtr.Zero);
            keybd_event(key, 0, 0, UIntPtr.Zero);
            keybd_event(key, 0, KeyeventfKeyup, UIntPtr.Zero);
            keybd_event(modifier, 0, KeyeventfKeyup, UIntPtr.Zero);
        }

        public void Dispose()
        {
            _disposed = true;
            _rawInput.Dispose();
            _tapTimer.Dispose();
            _scrollReady.Set();
            _scrollThread.Join(1000);
            _scrollReady.Dispose();
        }

        [DllImport("user32.dll")]
        private static extern void keybd_event(
            byte virtualKey,
            byte scanCode,
            uint flags,
            UIntPtr extraInfo);

        [DllImport("user32.dll")]
        private static extern void mouse_event(
            uint flags,
            uint dx,
            uint dy,
            int data,
            UIntPtr extraInfo);

        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool GetCursorPos(out ScreenPoint point);

        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool SetCursorPos(int x, int y);

        [StructLayout(LayoutKind.Sequential)]
        private struct ScreenPoint
        {
            public int X;
            public int Y;
        }
    }

    [StructLayout(LayoutKind.Explicit)]
    private struct BthLeUuidValue
    {
        [FieldOffset(0)] public ushort ShortUuid;
        [FieldOffset(0)] public Guid LongUuid;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct BthLeUuid
    {
        public byte IsShortUuid;
        private byte _padding1;
        private byte _padding2;
        private byte _padding3;
        public BthLeUuidValue Value;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct BthLeGattCharacteristic
    {
        public ushort ServiceHandle;
        public BthLeUuid CharacteristicUuid;
        public ushort AttributeHandle;
        public ushort CharacteristicValueHandle;
        public byte IsBroadcastable;
        public byte IsReadable;
        public byte IsWritable;
        public byte IsWritableWithoutResponse;
        public byte IsSignedWritable;
        public byte IsNotifiable;
        public byte IsIndicatable;
        public byte HasExtendedProperties;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct BthLeGattDescriptor
    {
        public ushort ServiceHandle;
        public ushort CharacteristicHandle;
        public int DescriptorType;
        public BthLeUuid DescriptorUuid;
        public ushort AttributeHandle;
    }

    [StructLayout(LayoutKind.Explicit, Size = 80)]
    private struct BthLeGattDescriptorValue
    {
        [FieldOffset(0)] public int DescriptorType;
        [FieldOffset(4)] public BthLeUuid DescriptorUuid;
        [FieldOffset(24)] public byte IsSubscribeToNotification;
        [FieldOffset(25)] public byte IsSubscribeToIndication;
        [FieldOffset(72)] public uint DataSize;
        [FieldOffset(76)] public byte Data;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct GattValueChangedRegistration
    {
        public ushort NumCharacteristics;
        private ushort _padding;
        public BthLeGattCharacteristic Characteristic;
    }

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate void GattEventCallback(
        int eventType,
        IntPtr eventOutParameter,
        IntPtr context);

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    private delegate bool ConsoleControlHandler(int controlType);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern SafeFileHandle CreateFileW(
        string fileName,
        uint desiredAccess,
        uint shareMode,
        IntPtr securityAttributes,
        uint creationDisposition,
        uint flagsAndAttributes,
        IntPtr templateFile);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetConsoleCtrlHandler(
        ConsoleControlHandler? handlerRoutine,
        bool add);

    [DllImport("BluetoothAPIs.dll")]
    private static extern int BluetoothGATTGetCharacteristics(
        SafeFileHandle device,
        IntPtr service,
        ushort characteristicsBufferCount,
        [Out] BthLeGattCharacteristic[]? characteristicsBuffer,
        out ushort characteristicsBufferActual,
        uint flags);

    [DllImport("BluetoothAPIs.dll")]
    private static extern int BluetoothGATTGetCharacteristicValue(
        SafeFileHandle device,
        ref BthLeGattCharacteristic characteristic,
        uint characteristicValueDataSize,
        IntPtr characteristicValue,
        out ushort characteristicValueSizeRequired,
        uint flags);

    [DllImport("BluetoothAPIs.dll")]
    private static extern int BluetoothGATTGetDescriptors(
        SafeFileHandle device,
        ref BthLeGattCharacteristic characteristic,
        ushort descriptorsBufferCount,
        [Out] BthLeGattDescriptor[]? descriptorsBuffer,
        out ushort descriptorsBufferActual,
        uint flags);

    [DllImport("BluetoothAPIs.dll")]
    private static extern int BluetoothGATTSetCharacteristicValue(
        SafeFileHandle device,
        ref BthLeGattCharacteristic characteristic,
        IntPtr characteristicValue,
        ulong reliableWriteContext,
        uint flags);

    [DllImport("BluetoothAPIs.dll")]
    private static extern int BluetoothGATTSetDescriptorValue(
        SafeFileHandle device,
        ref BthLeGattDescriptor descriptor,
        ref BthLeGattDescriptorValue descriptorValue,
        uint flags);

    [DllImport("BluetoothAPIs.dll")]
    private static extern int BluetoothGATTRegisterEvent(
        SafeFileHandle service,
        int eventType,
        ref GattValueChangedRegistration eventParameterIn,
        GattEventCallback callback,
        IntPtr callbackContext,
        out IntPtr eventHandle,
        uint flags);

    [DllImport("BluetoothAPIs.dll")]
    private static extern int BluetoothGATTUnregisterEvent(
        IntPtr eventHandle,
        uint flags);
}
