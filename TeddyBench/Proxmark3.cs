﻿using System;
using System.Collections.Generic;
using System.Drawing.Drawing2D;
using System.IO;
using System.IO.Ports;
using System.Linq;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using System.Windows.Forms;

namespace TeddyBench
{
    public class Proxmark3
    {
        public class Pm3UsbCommand
        {
            public Pm3UsbCommandStruct data = new Pm3UsbCommandStruct();

            public Pm3UsbCommand(ulong cmd = 0, ulong arg0 = 0, ulong arg1 = 0, ulong arg2 = 0, byte[] payload = null)
            {
                data = new Pm3UsbCommandStruct();
                data.arg = new ulong[3];
                data.d = new byte[512];

                data.cmd = cmd;
                data.arg[0] = arg0;
                data.arg[1] = arg1;
                data.arg[2] = arg2;
                SetData(payload);
            }

            public byte[] ToByteArray()
            {
                return StructureToByteArray(data);
            }

            internal void SetData(byte[] payload)
            {
                if (payload != null && payload.Length > 0)
                {
                    Array.Copy(payload, data.d, payload.Length);
                }
            }

            internal void Write(SerialPort p)
            {
                byte[] buf = ToByteArray();
                p.Write(buf, 0, buf.Length);
            }
        }

        public class Pm3UsbResponse
        {
            public enum eResponseType
            {
                NACK = 0xFE,
                ACK = 0xFF,
                DebugString = 0x100,
                DebugInteger = 0x101,
                DebugBytes = 0x102,
                MeasuredAntennaTuning = 0x410,
                Partial = 0x7FFD,
                NoData = 0x7FFE,
                Timeout = 0x7FFF
            }

            public Pm3UsbResponseStruct data;
            public bool VarSize => (data.cmd & 0x8000) != 0;
            public eResponseType Cmd => (eResponseType)(data.cmd & ~0x8000);
            public int Length => (ReceivedHeaderLength + ReceivedPayloadLength);
            public int ReceivedPayloadLength = 0;
            public int ReceivedHeaderLength = 0;
            private int ExpectedPayloadLength = 0;


            public Pm3UsbResponse(byte[] buf, int offset, int length)
            {
                ByteArrayToStructure(buf, offset, ref data);
            }

            public Pm3UsbResponse(SerialPort p, int waitTime = 1000)
            {
                try
                {
                    int loops = 0;
                    while ((p.BytesToRead < 16) && ((loops++ * 10) < waitTime))
                    {
                        Thread.Sleep(10);
                    }
                    if (p.BytesToRead < 16)
                    {
                        data.cmd = (int)eResponseType.NoData;
                        return;
                    }

                    byte[] buf = new byte[512 + 16];

                    if (p.Read(buf, 0, 16) != 16)
                    {
                        return;
                    }
                    ReceivedHeaderLength = 16;
                    ExpectedPayloadLength = 0;

                    ByteArrayToStructure(buf, 0, ref data);

                    if (VarSize)
                    {
                        ExpectedPayloadLength = data.dataLen;
                    }
                    else
                    {
                        ExpectedPayloadLength = 512;
                    }

                    if (ExpectedPayloadLength > 0)
                    {
                        ReceivedPayloadLength = p.Read(buf, 16, ExpectedPayloadLength);
                        if (ExpectedPayloadLength != ReceivedPayloadLength)
                        {
                            data.cmd = (int)eResponseType.Partial;
                            return;
                        }
                    }
                    ByteArrayToStructure(buf, 0, ref data);
                }
                catch (TimeoutException ex)
                {
                    data.cmd = (int)eResponseType.Timeout;
                    return;
                }
            }
        }

        public struct Pm3UsbCommandStruct
        {
            public ulong cmd;
            [MarshalAs(UnmanagedType.ByValArray, SizeConst = 3)]
            public ulong[] arg;
            [MarshalAs(UnmanagedType.ByValArray, SizeConst = 512)]
            public byte[] d;
        };

        public struct Pm3UsbResponseStruct
        {
            public ushort cmd;
            public ushort dataLen;
            [MarshalAs(UnmanagedType.ByValArray, SizeConst = 3)]
            public uint[] arg;
            [MarshalAs(UnmanagedType.ByValArray, SizeConst = 512)]
            public byte[] d;
        };

        private static byte[] StructureToByteArray(object obj)
        {
            int len = Marshal.SizeOf(obj);
            byte[] arr = new byte[len];
            IntPtr ptr = Marshal.AllocHGlobal(len);
            Marshal.StructureToPtr(obj, ptr, true);
            Marshal.Copy(ptr, arr, 0, len);
            Marshal.FreeHGlobal(ptr);

            return arr;
        }

        private static void ByteArrayToStructure<T>(byte[] bytearray, int offset, ref T obj)
        {
            int len = Marshal.SizeOf(obj);
            IntPtr i = Marshal.AllocHGlobal(len);
            Marshal.Copy(bytearray, offset, i, len);
            obj = (T)Marshal.PtrToStructure(i, typeof(T));
            Marshal.FreeHGlobal(i);
        }
        public string CurrentPort { get; private set; }
        public SerialPort Port { get; private set; }
        public event EventHandler<string> UidFound;
        public event EventHandler<string> DeviceFound;
        public string CurrentUidString = null;
        private Thread ScanThread = null;
        private bool ExitScanThread = false;
        private object ReaderLock = new object();

        private Dictionary<string, DateTime> PortsFailed = new Dictionary<string, DateTime>();
        private Dictionary<string, DateTime> PortsAppeared = new Dictionary<string, DateTime>();
        public bool UnlockSupported = false;

        public Proxmark3()
        {
            LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] new instance");
        }


        public void Start()
        {
            StartThread();
        }

        private void StartThread()
        {
            if (ScanThread == null)
            {
                ExitScanThread = false;
                ScanThread = new Thread(ScanThreadFunc);
                ScanThread.Name = "PM3 Scan Thread";
                ScanThread.Start();
            }
        }

        public void Stop()
        {
            StopThread();

            Close();

            DeviceFound?.Invoke(this, null);
        }

        public void StopThread()
        {
            if (ScanThread != null)
            {
                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Trying to stop thread");
                ExitScanThread = true;

                if (!ScanThread.Join(1000))
                {
                    LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Trying to abort thread");
                    ScanThread.Abort();
                }
                ScanThread = null;
            }
        }

        private void ScanThreadFunc()
        {
            lock (ReaderLock)
            {
                while (true)
                {
                    if (Port == null)
                    {
                        if (ExitScanThread)
                        {
                            LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MainFunc: Exiting");
                            return;
                        }
                        try
                        {
                            if (!Detect())
                            {
                                Thread.Sleep(250);
                            }
                        }
                        catch (Exception ex)
                        {
                            Close();
                            Thread.Sleep(250);
                        }
                    }
                    else
                    {
                        try
                        {
                            if (ExitScanThread)
                            {
                                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MainFunc: Exiting");
                                return;
                            }
                            Flush(Port);

                            byte[] rnd = GetRandom();

                            /* no SLIX-L found */
                            if (rnd == null)
                            {
                                if (CurrentUidString != null)
                                {
                                    CurrentUidString = null;
                                    UidFound?.Invoke(this, null);
                                }
                                Thread.Sleep(100);
                                continue;
                            }


                            LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MainFunc: Tag detected, identifying");

                            byte[] uid = GetUID();

                            /* no UID detected? might be locked */
                            if (uid == null)
                            {
                                if (UnlockSupported)
                                {
                                    LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MainFunc: Tag in privacy mode, try to unlock");

                                    /* try to unlock using the common passwords */
                                    if (UnlockTag(0x0F0F0F0F) || UnlockTag(0x7FFD6E5B))
                                    {
                                        LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MainFunc: Unlocked tag");
                                        uid = GetUID();
                                        if(uid == null)
                                        {
                                            LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MainFunc: But it did still not respond to an INVENTORY command?!");
                                            Thread.Sleep(100);
                                            continue;
                                        }
                                    }
                                }
                                else
                                {
                                    LogWindow.Log(LogWindow.eLogLevel.Warning, "[PM3] MainFunc: Tag in privacy mode, unlock not supported by your firmware");
                                    LogWindow.Log(LogWindow.eLogLevel.Warning, "[PM3] MainFunc: Either use our custom firmware from");
                                    LogWindow.Log(LogWindow.eLogLevel.Warning, "[PM3] MainFunc: -> https://github.com/g3gg0/proxmark3/tree/iso15693_slix_l_features");
                                    LogWindow.Log(LogWindow.eLogLevel.Warning, "[PM3] MainFunc: or use the \"knock method\" to get it out of privacy mode ");
                                    LogWindow.Log(LogWindow.eLogLevel.Warning, "[PM3] MainFunc: -> https://youtu.be/IiZYM5k90pY");
                                    LogWindow.Log(LogWindow.eLogLevel.Warning, "[PM3] MainFunc: Please remove the tag");

                                    while (GetRandom() != null)
                                    {
                                        /* exit as quickly as possible if requested */
                                        if (ExitScanThread)
                                        {
                                            LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MainFunc: Exiting");
                                            Flush(Port);
                                            return;
                                        }
                                        Thread.Sleep(100);
                                    }
                                    LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MainFunc: Tag disappeared");
                                }
                            }

                            /* exit as quickly as possible if requested */
                            if (ExitScanThread)
                            {
                                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MainFunc: Exiting");
                                Flush(Port);
                                return;
                            }

                            string uidString = UIDToString(uid);

                            /* found a new one? print a log message */
                            if (uidString != null)
                            {
                                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Report tag UID: " + uidString);
                            }
                            else
                            {
                                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Report removed tag");
                            }
                            CurrentUidString = uidString;

                            /* notify listeners about currently detected UID */
                            UidFound?.Invoke(this, uidString);


                            while (GetRandom(uid) != null)
                            {
                                /* exit as quickly as possible if requested */
                                if (ExitScanThread)
                                {
                                    LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MainFunc: Exiting");
                                    Flush(Port);
                                    return;
                                }
                                Thread.Sleep(100);
                            }
                            LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MainFunc: Tag disappeared");

                        }
                        catch (InvalidOperationException ex2)
                        {
                            if (ex2.TargetSite.DeclaringType.Name == "SerialPort")
                            {
                                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MainFunc: Serial port lost. Closing device.");
                            }
                            else
                            {
                                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MainFunc: Device failed. Closing. (" + ex2.ToString() + ")");
                            }
                            Close();
                        }
                        catch (Exception ex3)
                        {
                            LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MainFunc: Device failed. Closing. (" + ex3.ToString() + ")");
                            Close();
                        }
                    }
                    Thread.Sleep(50);
                }
            }
        }

        private void Close()
        {
            lock (this)
            {
                if (Port != null)
                {
                    try
                    {
                        Flush(Port);
                    }
                    catch (Exception ex)
                    {
                    }

                    try
                    {
                        Port.Close();
                    }
                    catch (Exception ex)
                    {
                    }
                    Port.Dispose();
                    Port = null;
                    CurrentPort = null;
                }
                DeviceFound?.Invoke(this, CurrentPort);
            }
        }

        private bool UnlockTag(uint pass)
        {
            int reason = 0;
            return UnlockTag(pass, ref reason);
        }

        private bool UnlockTag(uint pass, ref int reason)
        {
            if (Port == null)
            {
                return false;
            }
            if (!UnlockSupported)
            {
                return false;
            }
            Pm3UsbCommand cmd = new Pm3UsbCommand(0x319, pass);

            LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] UnlockTag: Send request for pass 0x" + pass.ToString("X8"));
            cmd.Write(Port);

            while (true)
            {
                Pm3UsbResponse response = new Pm3UsbResponse(Port);

                switch (response.Cmd)
                {
                    case Pm3UsbResponse.eResponseType.DebugString:
                        {
                            string debugStr = Encoding.UTF8.GetString(response.data.d, 0, response.data.dataLen);
                            LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] UnlockTag:   String '" + debugStr + "'");
                            break;
                        }

                    case Pm3UsbResponse.eResponseType.ACK:
                        reason = 0;
                        LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] UnlockTag: ACK");
                        return true;

                    case Pm3UsbResponse.eResponseType.NACK:
                        reason = 1 + (int)response.data.arg[0];
                        LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] UnlockTag: NACK (reason: " + reason + ")");
                        return false;
                
                    case Pm3UsbResponse.eResponseType.NoData:
                    case Pm3UsbResponse.eResponseType.Timeout:
                        reason = -1;
                        LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] UnlockTag: timeout, returning");
                        return false;

                    default:
                        LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] UnlockTag: Unhandled: " + response.Cmd);
                        break;
                }
            }
        }

        private string UIDToString(byte[] uid)
        {
            if (uid != null)
            {
                return BitConverter.ToString(uid.Reverse().ToArray()).Replace("-", "");
            }
            return null;
        }

        private byte[] SendCommand(byte[] command)
        {
            if (Port == null)
            {
                return null;
            }

            Pm3UsbCommand cmd = new Pm3UsbCommand(0x313, (byte)command.Length, 1, 1, command);

            LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] GetResponse: Send " + BitConverter.ToString(command).Replace("-", ""));
            cmd.Write(Port);

            while (true)
            {
                Pm3UsbResponse response = new Pm3UsbResponse(Port);

                switch (response.Cmd)
                {
                    case Pm3UsbResponse.eResponseType.DebugString:
                        {
                            string debugStr = Encoding.UTF8.GetString(response.data.d, 0, response.data.dataLen);
                            LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] GetResponse: DebugMessage '" + debugStr + "'");
                            break;
                        }

                    case Pm3UsbResponse.eResponseType.NoData:
                    case Pm3UsbResponse.eResponseType.Timeout:
                        LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] GetResponse: timeout, returning");
                        return null;

                    case Pm3UsbResponse.eResponseType.ACK:
                        if (response.data.arg[0] > 0 && response.data.arg[0] < response.data.d.Length)
                        {
                            byte[] ret = new byte[response.data.arg[0]];
                            Array.Copy(response.data.d, ret, response.data.arg[0]);

                            if (!ISO15693.CheckChecksum(ret))
                            {
                                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] GetResponse: ACK, but Checksum failed");
                                return null;
                            }

                            LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] GetResponse: ACK, returning data");
                            return ret;
                        }
                        else
                        {
                            LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] GetResponse: no tag answered, returning");
                            return null;
                        }

                    default:
                        LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] GetResponse: Unhandled: " + response.Cmd);
                        break;
                }
            }
        }

        #region Send ISO15693 commands

        private byte[] GetRandom(byte[] uid = null)
        {
            byte[] cmdIdentify = ISO15693.BuildCommand(ISO15693.Command.NXP_GET_RANDOM_NUMBER, uid, null);
            byte[] data = SendCommand(cmdIdentify);

            if (data == null || data.Length != 5)
            {
                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] GetRandom: Failed (" + ((data == null) ? "no resp" : (data.Length + " bytes")) + ")");
                return null;
            }

            byte[] rnd = new byte[2];
            Array.Copy(data, 1, rnd, 0, 2);

            return rnd;
        }

        private byte[] GetUID(int tries = 1)
        {
            for (int tried = 0; tried < tries; tried++)
            {
                byte[] cmdIdentify = ISO15693.BuildCommand(ISO15693.Command.INVENTORY, new byte[1]);
                byte[] data = SendCommand(cmdIdentify);

                if (data == null || data.Length != 12)
                {
                    LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] GetUID: Failed (" + ((data == null) ? "no resp" : (data.Length + " bytes")) + ")");
                    continue;
                }

                byte[] uidBytes = new byte[8];
                Array.Copy(data, 2, uidBytes, 0, 8);

                string uid = BitConverter.ToString(uidBytes.Reverse().ToArray()).Replace("-", "");
                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] GetUID: ACK, returning " + uid.Length + " byte");
                return uidBytes;
            }
            return null;
        }

        private byte[] ReadMemory(int bank, int tries = 1)
        {
            for (int tried = 0; tried < tries; tried++)
            {
                byte[] cmdReadMem = ISO15693.BuildCommand(ISO15693.Command.READBLOCK, new[] { (byte)bank });
                byte[] data = SendCommand(cmdReadMem);

                if (data == null || data.Length != 7)
                {
                    LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] ReadMemory: Failed (" + ((data == null) ? "no resp" : (data.Length + " bytes")) + ")");
                    continue;
                }

                byte[] mem = new byte[4];
                Array.Copy(data, 1, mem, 0, 4);

                string str = BitConverter.ToString(mem.ToArray()).Replace("-", "");
                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] ReadMemory: ACK, returning " + str + "");
                return mem;
            }
            return null;
        }

        #endregion

        private void EmulateTagInternal(byte[] data)
        {
            if(Port == null)
            {
                return;
            }
            Pm3UsbCommand cmd = new Pm3UsbCommand(0x311, 0, 0, 0, data);

            LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Emulate: Start");
            cmd.Write(Port);

            while (true)
            {
                Pm3UsbResponse response = new Pm3UsbResponse(Port);

                switch (response.Cmd)
                {
                    case Pm3UsbResponse.eResponseType.DebugString:
                        {
                            string debugStr = Encoding.UTF8.GetString(response.data.d, 0, response.data.dataLen);
                            LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Emulate: DebugMessage '" + debugStr + "'");
                            break;
                        }

                    case Pm3UsbResponse.eResponseType.NoData:
                    case Pm3UsbResponse.eResponseType.Timeout:
                        //LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Emulate: timeout, returning");
                        //return;
                        continue;

                    case Pm3UsbResponse.eResponseType.ACK:
                        LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Emulate: Done");
                        return;

                    default:
                        LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Emulate: Unhandled: " + response.Cmd);
                        break;
                }
            }
        }

        public class MeasurementResult
        {
            public float vLF125;
            public float vLF134;
            public uint peakF;
            public float peakV;
            public byte[] amplitudes = new byte[256];

            internal double[] GetFrequencieskHz()
            {
                double[] freq = new double[256];

                for(int pos = 0; pos < freq.Length; pos++)
                {
                    freq[pos] = (12000000.0f / (pos + 1)) / 1000.0f;
                }
                return freq;
            }

            internal double GetPeakFrequency()
            {
                return 12000000 / (peakF + 1);
            }

            internal double[] GetVoltages()
            {
                return amplitudes.Select(b => (double)(b * 512.0f / 1000.0f)).ToArray();
            }
        }

        private bool MeasureAntennaInternal(MeasurementResult result)
        {
            if (Port == null)
            {
                return;
            }
            Pm3UsbCommand cmd = new Pm3UsbCommand(0x400, 1);

            LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MeasureAntenna: Start");
            cmd.Write(Port);

            while (true)
            {
                Pm3UsbResponse response = new Pm3UsbResponse(Port);

                switch (response.Cmd)
                {
                    case Pm3UsbResponse.eResponseType.DebugString:
                        {
                            string debugStr = Encoding.UTF8.GetString(response.data.d, 0, response.data.dataLen);
                            LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MeasureAntenna: DebugMessage '" + debugStr + "'");
                            break;
                        }

                    case Pm3UsbResponse.eResponseType.NoData:
                    case Pm3UsbResponse.eResponseType.Timeout:
                        LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MeasureAntenna: timeout");
                        continue;

                    case Pm3UsbResponse.eResponseType.MeasuredAntennaTuning:
                        result.vLF125 = (response.data.arg[0] & 0xFFFF) * 0.002f;
                        result.vLF134 = ((response.data.arg[0] >> 16) & 0xFFFF) * 0.002f;
                        result.peakF = (response.data.arg[2] & 0xFFFF);
                        result.peakV = ((response.data.arg[0] >> 16) & 0xFFFF) * 0.002f;
                        Array.Copy(response.data.d, result.amplitudes, 256);

                        return true;

                    default:
                        LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] MeasureAntenna: Unhandled: " + response.Cmd);
                        break;
                }
            }
        }

        private void Flush(SerialPort p)
        {
            if (p.BytesToRead > 0)
            {
                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Flush: " + p.BytesToRead + " bytes to flush");
            }
            while (p.BytesToRead > 0)
            {
                p.ReadByte();
            }
        }

        public bool Detect()
        {
            string[] ports = SerialPort.GetPortNames();
            List<string> reallyAvailablePorts = new List<string>();

            /* SerialPort.GetPortNames() also returns stale ports from registry. do a check if they are really available. */
            foreach (string listedPort in ports)
            {
                try
                {
                    SerialPort p = new SerialPort(listedPort, 115200);

                    p.Open();
                    p.Close();
                    reallyAvailablePorts.Add(listedPort);
                }
                catch(Exception e0)
                {
                }
            }

            /* update seen list */
            foreach(string newPort in reallyAvailablePorts)
            {
                if(!PortsAppeared.ContainsKey(newPort))
                {
                    PortsAppeared.Add(newPort, DateTime.Now);
                    LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] New port: " + newPort);
                }
            }

            /* remove failed ports when they disappear */
            foreach (string del in PortsFailed.Keys.Where(p => !reallyAvailablePorts.Contains(p)).ToArray())
            {
                PortsFailed.Remove(del);
                PortsAppeared.Remove(del);
                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Port disappeared: " + del);
            }

            /* only try to connect to ports that have been seen more than a second ago. Ensure PM3 has booted properly. */
            foreach(string port in reallyAvailablePorts.Where(p=> (DateTime.Now - PortsAppeared[p]).TotalMilliseconds > 1000))
            {
                /* retry failed ports only every 60 seconds */
                if(PortsFailed.ContainsKey(port) && (DateTime.Now - PortsFailed[port]).TotalSeconds < 60)
                {
                    continue;
                }

                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Try port " + port);
                if (TryOpen(port))
                {
                    LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Success");
                    return true;
                }

                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Failed");
                if (!PortsFailed.ContainsKey(port))
                {
                    PortsFailed.Add(port, DateTime.Now);
                }
                PortsFailed[port] = DateTime.Now;
            }

            return false;
        }

        private bool TryOpen(string port)
        {
            SerialPort p = new SerialPort(port, 115200);
            try
            {
                p = new SerialPort(port, 115200);
                p.ReadTimeout = 500;

                p.Open();
                Flush(p);

                Pm3UsbCommand cmdPing = new Pm3UsbCommand(0x109);
                cmdPing.Write(p);
                Pm3UsbResponse resPing = new Pm3UsbResponse(p);

                if (resPing.Cmd != Pm3UsbResponse.eResponseType.ACK)
                {
                    LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] TryOpen: "+ port + " did not reply to a ping");
                    p.Close();
                    return false;
                }

                /* read version */
                Pm3UsbCommand cmdVers = new Pm3UsbCommand(0x107);
                cmdVers.Write(p);
                Pm3UsbResponse resVers = new Pm3UsbResponse(p);
                if (resVers.Cmd != Pm3UsbResponse.eResponseType.ACK)
                {
                    LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] TryOpen: " + port + " did not reply to a version query. (got " + resVers.Cmd.ToString() + " instead, " + resVers.Length + " bytes)");
                    p.Close();
                    return false;
                }

                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Chip ID: 0x" + resVers.data.arg[0].ToString("X8"));
                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Flash:   0x" + resVers.data.arg[1].ToString("X8"));
                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Caps:    0x" + resVers.data.arg[2].ToString("X8"));
                string versionString = Encoding.UTF8.GetString(resVers.data.d, 0, resVers.data.dataLen - 1);
                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Version:");
                foreach (string line in versionString.Split('\n'))
                {
                    LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3]     " + line);
                }

                CurrentPort = port;
                Port = p;

                /* does it support the unlock command? */
                int reason = 0;
                UnlockSupported = true;
                UnlockTag(0xDEADBEEF, ref reason);

                /* if not supported, the command will simply time out */
                UnlockSupported = reason > 0;
                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] Device does " + (UnlockSupported ? "" : "*NOT*") + " support SLIX-L unlock command");

                DeviceFound?.Invoke(this, CurrentPort);
                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] TryOpen: " + port + " successfully opened");
                return true;
            }
            catch(Exception ex)
            {
                try
                {
                    if (p != null && p.IsOpen)
                    {
                        p.Close();
                    }
                }
                catch(Exception e)
                {
                }
            }

            return false;
        }

        private byte[] ReadMemoryInternal()
        {
            byte[] mem = new byte[8 * 4];
            byte[] uid = null;

            lock (ReaderLock)
            {
                uid = GetUID(100);

                if (uid == null)
                {
                    return null;
                }

                for (int bank = 0; bank < 8; bank++)
                {
                    byte[] buf = ReadMemory(bank, 100);

                    if (buf == null)
                    {
                        LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] ReadMemory: Failed to read bank " + bank + ", aborting");
                        return null;
                    }
                    Array.Copy(buf, 0, mem, bank * 4, buf.Length);
                }
            }

            for (int bank = 0; bank < 8; bank++)
            {
                byte[] buf = new byte[4];

                Array.Copy(mem, bank * 4, buf, 0, 4);
                string str = BitConverter.ToString(buf.ToArray()).Replace("-", " ");
                LogWindow.Log(LogWindow.eLogLevel.Debug, "[PM3] ReadMemory: #" + bank + "  " + str);
            }

            byte[] data = new byte[8 + 8 * 4];

            Array.Copy(uid.Reverse().ToArray(), data, 8);
            Array.Copy(mem, 0, data, 8, 8 * 4);

            return data;
        }

        internal byte[] ReadMemory()
        {
            if (Port == null)
            {
                return null;
            }
            byte[] ret = null;

            StopThread();
            try
            {
                lock (ReaderLock)
                {
                    ret = ReadMemoryInternal();
                }
            }
            catch (ThreadAbortException ex)
            {
                StartThread();
                throw ex;
            }

            StartThread();

            return ret;
        }

        internal void EmulateTag(byte[] data)
        {
            if (Port == null)
            {
                return;
            }

            StopThread();
            lock (ReaderLock)
            {
                EmulateTagInternal(data);
            }
            StartThread();
        }

        internal MeasurementResult MeasureAntenna()
        {
            MeasurementResult result = new MeasurementResult();
            if (Port == null)
            {
                return null;
            }

            StopThread();
            lock (ReaderLock)
            {
                MeasureAntennaInternal(result);
            }
            StartThread();

            return result;
        }
    }
}
