# Dot-source from the existing bounded Windows Actions signing controller.
# Development pin/integrity verification is deliberately NOT public publisher trust.
Set-StrictMode -Version Latest

function Initialize-DevelopmentAuthenticode {
    if (-not $IsWindows -or $env:GITHUB_ACTIONS -ne 'true') {
        throw 'Development Authenticode verification requires Windows GitHub Actions.'
    }
    if ($PSVersionTable.PSVersion.Major -lt 7) { throw 'PowerShell 7 is required.' }
    Add-Type -AssemblyName System.Security.Cryptography.Pkcs -ErrorAction Stop
    if ('Nvsf.DevelopmentAuthenticode' -as [type]) { return }
    # Add-Type compiles this platform glue ONLY on the hosted Windows path above.
    Add-Type -ErrorAction Stop -TypeDefinition @'
using System;
using System.IO;
using System.Runtime.InteropServices;

namespace Nvsf {
    public static class DevelopmentAuthenticode {
        [StructLayout(LayoutKind.Sequential)]
        struct FileInfo {
            public uint Size;
            public IntPtr Path, Handle, KnownSubject;
        }
        [StructLayout(LayoutKind.Sequential)]
        struct SignatureSettings {
            public uint Size, Index, Flags, Secondary, VerifiedIndex;
            public IntPtr CryptoPolicy;
        }
        [StructLayout(LayoutKind.Sequential)]
        struct TrustData {
            public uint Size;
            public IntPtr Policy, Sip;
            public uint UI, Revocation, Choice;
            public IntPtr File;
            public uint Action;
            public IntPtr State, Url;
            public uint Flags, Context;
            public IntPtr Signatures;
        }
        [DllImport("wintrust.dll", ExactSpelling=true)]
        static extern int WinVerifyTrust(IntPtr window, ref Guid action, ref TrustData data);
        [DllImport("crypt32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        static extern bool CryptQueryObject(uint type, string path, uint content, uint format,
            uint flags, out uint encoding, out uint actualContent, out uint actualFormat,
            out IntPtr store, out IntPtr message, IntPtr context);
        [DllImport("crypt32.dll", SetLastError=true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        static extern bool CryptMsgGetParam(IntPtr message, uint parameter, uint index,
            [Out] byte[] data, ref uint length);
        [DllImport("crypt32.dll", SetLastError=true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        static extern bool CryptMsgClose(IntPtr message);
        [DllImport("crypt32.dll", SetLastError=true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        static extern bool CertCloseStore(IntPtr store, uint flags);

        public static int[] Verify(string path, IntPtr handle, bool hashOnly) {
            if (IntPtr.Size != 8 || Marshal.SizeOf<FileInfo>() != 32 ||
                Marshal.SizeOf<TrustData>() != 88 || Marshal.SizeOf<SignatureSettings>() != 32)
                throw new InvalidOperationException("Unsupported Windows verification ABI.");
            IntPtr name = IntPtr.Zero, file = IntPtr.Zero, signatures = IntPtr.Zero;
            var data = new TrustData();
            var action = new Guid("00AAC56B-CD44-11D0-8CC2-00C04FC295EE");
            bool invoked = false;
            try {
                name = Marshal.StringToCoTaskMemUni(path);
                file = Marshal.AllocHGlobal(32);
                signatures = Marshal.AllocHGlobal(32);
                Marshal.StructureToPtr(new FileInfo { Size=32, Path=name, Handle=handle }, file, false);
                // WSS_VERIFY_SPECIFIC | WSS_GET_SECONDARY_SIG_COUNT, primary only.
                Marshal.StructureToPtr(new SignatureSettings { Size=32, Flags=3 }, signatures, false);
                data.Size=88; data.UI=2; data.Choice=1; data.File=file; data.Action=1;
                // Offline, no revocation service, disable MD2/MD4. No global policy changes.
                data.Flags=0x1000 | 0x2000 | (hashOnly ? 0x200u : 0u);
                data.Signatures=signatures;
                invoked=true;
                int result=WinVerifyTrust(new IntPtr(-1), ref action, ref data);
                var setting=Marshal.PtrToStructure<SignatureSettings>(signatures);
                return new int[] { result, checked((int)setting.Secondary), checked((int)setting.VerifiedIndex) };
            } finally {
                int closed=0;
                try {
                    if (invoked) { data.Action=2; closed=WinVerifyTrust(new IntPtr(-1), ref action, ref data); }
                } finally {
                    if (signatures != IntPtr.Zero) Marshal.FreeHGlobal(signatures);
                    if (file != IntPtr.Zero) Marshal.FreeHGlobal(file);
                    if (name != IntPtr.Zero) Marshal.FreeCoTaskMem(name);
                }
                if (closed != 0) throw new InvalidOperationException("WinTrust state cleanup failed.");
            }
        }
        public static byte[] EmbeddedMessage(string path) {
            IntPtr store=IntPtr.Zero, message=IntPtr.Zero;
            try {
                uint encoding, content, format;
                // FILE=1; SIGNED_EMBED=10; BINARY=1. Never catalog/general/PFX fallback.
                if (!CryptQueryObject(1, path, 1u << 10, 1u << 1, 0,
                    out encoding, out content, out format, out store, out message, IntPtr.Zero) ||
                    content != 10 || format != 1 || message == IntPtr.Zero)
                    throw new InvalidOperationException("Embedded Authenticode query failed.");
                uint size=0;
                if (!CryptMsgGetParam(message, 5, 0, null, ref size) || size != 4)
                    throw new InvalidOperationException("CMS signer count query failed.");
                byte[] count=new byte[4];
                if (!CryptMsgGetParam(message, 5, 0, count, ref size) || size != 4 ||
                    BitConverter.ToUInt32(count,0) != 1)
                    throw new InvalidOperationException("Exactly one CMS signer is required.");
                size=0;
                if (!CryptMsgGetParam(message, 29, 0, null, ref size) || size == 0 || size > 1048576)
                    throw new InvalidOperationException("Encoded CMS exceeds its byte bound.");
                byte[] blob=new byte[size];
                uint actual=size;
                if (!CryptMsgGetParam(message,29,0,blob,ref actual) || actual != size)
                    throw new InvalidOperationException("Encoded CMS read failed.");
                return blob;
            } finally {
                bool a=message == IntPtr.Zero || CryptMsgClose(message);
                bool b=store == IntPtr.Zero || CertCloseStore(store,0);
                if (!a || !b) throw new InvalidOperationException("Embedded message cleanup failed.");
            }
        }
        static void Range(byte[] b, long offset, long length) {
            if (offset < 0 || length < 0 || offset > b.LongLength || length > b.LongLength-offset)
                throw new InvalidDataException("PE range is outside the file.");
        }
        static uint U32(byte[] b, long offset) { Range(b,offset,4); return BitConverter.ToUInt32(b,(int)offset); }
        static ushort U16(byte[] b, long offset) { Range(b,offset,2); return BitConverter.ToUInt16(b,(int)offset); }
        public static long[] Layout(byte[] b) {
            if (b.Length < 64 || b.Length > 268435456 || U16(b,0) != 0x5a4d)
                throw new InvalidDataException("Invalid bounded PE image.");
            long pe=U32(b,60); Range(b,pe,24);
            if (pe < 64 || U32(b,pe) != 0x4550) throw new InvalidDataException("Invalid PE signature.");
            int sections=U16(b,pe+6), optionalSize=U16(b,pe+20);
            if (sections < 1 || sections > 96) throw new InvalidDataException("Invalid PE section count.");
            long optional=pe+24; Range(b,optional,optionalSize);
            ushort magic=U16(b,optional);
            int directories=magic == 0x20b ? 112 : magic == 0x10b ? 96 : -1;
            if (directories < 0 || optionalSize < directories+40)
                throw new InvalidDataException("Unsupported PE optional header.");
            uint count=U32(b,optional+directories-4);
            if (count < 5 || count > 16 || optionalSize < directories+8*count)
                throw new InvalidDataException("Invalid PE data-directory count.");
            long headers=U32(b,optional+60), sectionTable=optional+optionalSize;
            Range(b,sectionTable,40L*sections);
            if (headers < sectionTable+40L*sections || headers > b.Length)
                throw new InvalidDataException("Invalid PE header extent.");
            long end=headers;
            for (int i=0;i<sections;i++) {
                long entry=sectionTable+40L*i;
                long length=U32(b,entry+16), start=U32(b,entry+20);
                if (length == 0) continue;
                Range(b,start,length);
                if (start < headers) throw new InvalidDataException("PE section overlaps headers.");
                for (int j=0;j<i;j++) {
                    long other=sectionTable+40L*j, n=U32(b,other+16), p=U32(b,other+20);
                    if (n != 0 && start < p+n && p < start+length)
                        throw new InvalidDataException("PE sections overlap.");
                }
                end=Math.Max(end,start+length);
            }
            long directory=optional+directories+32;
            long certificate=U32(b,directory), tableSize=U32(b,directory+4);
            if ((certificate == 0) != (tableSize == 0))
                throw new InvalidDataException("Incomplete PE certificate directory.");
            long messageSize=0;
            if (certificate != 0) {
                Range(b,certificate,tableSize);
                if (certificate < end || certificate % 8 != 0 || tableSize < 8 ||
                    certificate+tableSize != b.Length)
                    throw new InvalidDataException("Certificate table must be aligned and end exactly at EOF.");
                long record=U32(b,certificate);
                if (record < 10 || record > 1048584 || ((record+7)&~7L) != tableSize ||
                    U16(b,certificate+4) != 0x200 || U16(b,certificate+6) != 2)
                    throw new InvalidDataException("Exactly one PKCS7 WIN_CERTIFICATE is required.");
                for (long i=certificate+record;i<b.Length;i++)
                    if (b[i] != 0) throw new InvalidDataException("Nonzero certificate alignment padding.");
                messageSize=record-8;
            }
            return new long[] { optional+64, directory, certificate, tableSize, messageSize };
        }
        public static byte[] Unsigned(byte[] b) {
            long[] p=Layout(b);
            byte[] result=new byte[p[2] == 0 ? b.Length : p[2]];
            Array.Copy(b,result,result.Length);
            Array.Clear(result,(int)p[0],4); Array.Clear(result,(int)p[1],8);
            return result;
        }
        public static byte[] Certificate(byte[] b) {
            long[] p=Layout(b);
            if (p[2] == 0) throw new InvalidDataException("Embedded signature is missing.");
            byte[] result=new byte[p[4]];
            Array.Copy(b,p[2]+8,result,0,result.Length);
            // DER outer SEQUENCE must consume the payload; SignTool may count the 8-byte
            // alignment inside dwLength, so only fewer than 8 trailing zero bytes are tolerated.
            int cursor=0; ReadDer(result,ref cursor,0x30);
            if (result.Length-cursor >= 8) throw new InvalidDataException("Trailing CMS bytes are not permitted.");
            for (int i=cursor;i<result.Length;i++)
                if (result[i] != 0) throw new InvalidDataException("Trailing CMS bytes are not permitted.");
            if (cursor == result.Length) return result;
            byte[] der=new byte[cursor];
            Array.Copy(result,der,cursor);
            return der;
        }
        static byte[] ReadDer(byte[] b, ref int cursor, int tag) {
            Range(b,cursor,2);
            if (b[cursor++] != tag) throw new InvalidDataException("Unexpected Authenticode ASN.1 tag.");
            int length=b[cursor++];
            if ((length & 128) != 0) {
                int n=length&127; length=0;
                if (n < 1 || n > 3) throw new InvalidDataException("Unsupported ASN.1 length.");
                Range(b,cursor,n);
                if (b[cursor] == 0) throw new InvalidDataException("Nonminimal ASN.1 length.");
                for (int i=0;i<n;i++) length=checked(length*256+b[cursor++]);
                if (length < 128) throw new InvalidDataException("Nonminimal ASN.1 length.");
            }
            Range(b,cursor,length); byte[] result=new byte[length];
            Array.Copy(b,cursor,result,0,length); cursor+=length; return result;
        }
        public static void RequireSha256IndirectData(byte[] content) {
            int i=0; byte[] sequence=ReadDer(content,ref i,0x30);
            if (i != content.Length) throw new InvalidDataException("Trailing indirect data.");
            i=0; ReadDer(sequence,ref i,0x30); byte[] digest=ReadDer(sequence,ref i,0x30);
            if (i != sequence.Length) throw new InvalidDataException("Invalid indirect-data fields.");
            i=0; byte[] algorithm=ReadDer(digest,ref i,0x30); byte[] hash=ReadDer(digest,ref i,4);
            if (i != digest.Length || hash.Length != 32) throw new InvalidDataException("Invalid PE SHA256 digest.");
            i=0; byte[] oid=ReadDer(algorithm,ref i,6);
            byte[] sha256={0x60,0x86,0x48,0x01,0x65,0x03,0x04,0x02,0x01};
            if (!Convert.ToHexString(oid).Equals(Convert.ToHexString(sha256),StringComparison.Ordinal))
                throw new InvalidDataException("PE digest algorithm must be SHA256.");
            if (i != algorithm.Length) {
                byte[] empty=ReadDer(algorithm,ref i,5);
                if (empty.Length != 0 || i != algorithm.Length) throw new InvalidDataException("Invalid digest parameters.");
            }
        }
    }
}
'@
}

function Get-UnsignedPeBytes {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$Path)
    Initialize-DevelopmentAuthenticode
    $stream = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        if ($stream.Length -gt 268435456) { throw 'PE image exceeds its byte bound.' }
        $bytes = [byte[]]::new([int]$stream.Length)
        $stream.ReadExactly($bytes)
        return ,([Nvsf.DevelopmentAuthenticode]::Unsigned($bytes))
    } finally { $stream.Dispose() }
}

function Assert-DevelopmentSignature {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][ValidatePattern('^[0-9a-fA-F]{64}$')][string]$CertificateSha256
    )
    Initialize-DevelopmentAuthenticode
    $resolved = [IO.Path]::GetFullPath($Path)
    $item = Get-Item -LiteralPath $resolved -Force -ErrorAction Stop
    if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw 'A regular signature input is required.'
    }
    # Keep write/delete sharing denied throughout native query, CMS, hash and trust checks.
    $stream = [IO.File]::Open($resolved, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    $cert = $null
    try {
        if ($stream.Length -lt 64 -or $stream.Length -gt 268435456) { throw 'PE image exceeds its byte bound.' }
        $bytes = [byte[]]::new([int]$stream.Length)
        $stream.ReadExactly($bytes)
        $embedded = [Nvsf.DevelopmentAuthenticode]::Certificate($bytes)
        $queried = [Nvsf.DevelopmentAuthenticode]::EmbeddedMessage($resolved)
        if ([Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($embedded)) -cne
            [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($queried))) {
            throw 'Native CMS query differs from the exact PE signature blob.'
        }
        $cms = [Security.Cryptography.Pkcs.SignedCms]::new()
        $cms.Decode($embedded)
        if ($cms.Detached -or $cms.SignerInfos.Count -ne 1 -or
            $cms.ContentInfo.ContentType.Value -cne '1.3.6.1.4.1.311.2.1.4') {
            throw 'Exactly one embedded Authenticode signer is required.'
        }
        $signer = $cms.SignerInfos[0]
        if ($signer.CounterSignerInfos.Count -ne 0 -or $signer.UnsignedAttributes.Count -ne 0 -or
            $signer.DigestAlgorithm.Value -cne '2.16.840.1.101.3.4.2.1') {
            throw 'Only an untimestamped single SHA256 signature is permitted.'
        }
        [Nvsf.DevelopmentAuthenticode]::RequireSha256IndirectData($cms.ContentInfo.Content)
        $cms.CheckSignature($true)
        $cert = $signer.Certificate
        if ($null -eq $cert) { throw 'CMS signer certificate is missing.' }
        $pin = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($cert.RawData)).ToLowerInvariant()
        if ($pin -cne $CertificateSha256.ToLowerInvariant()) { throw 'Development signer pin differs.' }
        $now = [DateTime]::UtcNow
        if ($now -lt $cert.NotBefore.ToUniversalTime() -or $now -gt $cert.NotAfter.ToUniversalTime()) {
            throw 'Development signer certificate is outside its validity period.'
        }
        $eku = @($cert.Extensions | Where-Object { $_.Oid.Value -ceq '2.5.29.37' })
        $usage = @($cert.Extensions | Where-Object { $_.Oid.Value -ceq '2.5.29.15' })
        $basic = @($cert.Extensions | Where-Object { $_.Oid.Value -ceq '2.5.29.19' })
        if ($eku.Count -ne 1 -or $usage.Count -ne 1 -or $basic.Count -ne 1) {
            throw 'Explicit signer EKU, key usage and basic constraints are required.'
        }
        $typedEku = [Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new($eku[0], $eku[0].Critical)
        $typedUsage = [Security.Cryptography.X509Certificates.X509KeyUsageExtension]::new($usage[0], $usage[0].Critical)
        $typedBasic = [Security.Cryptography.X509Certificates.X509BasicConstraintsExtension]::new($basic[0], $basic[0].Critical)
        if ('1.3.6.1.5.5.7.3.3' -cnotin @($typedEku.EnhancedKeyUsages | ForEach-Object { $_.Value }) -or
            -not ($typedUsage.KeyUsages -band [Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature) -or
            $typedBasic.CertificateAuthority) { throw 'Certificate is not a non-CA code-signing identity.' }
        $hash = [Nvsf.DevelopmentAuthenticode]::Verify($resolved, $stream.SafeFileHandle.DangerousGetHandle(), $true)
        if ($hash[0] -ne 0 -or $hash[1] -ne 0 -or $hash[2] -ne 0) {
            throw 'Native Authenticode file-digest verification failed.'
        }
        $trust = [Nvsf.DevelopmentAuthenticode]::Verify($resolved, $stream.SafeFileHandle.DangerousGetHandle(), $false)
        # CERT_E_UNTRUSTEDROOT is an expected separate NEGATIVE, never a success exemption.
        if ($trust[0] -ne -2146762487 -or $trust[1] -ne 0) {
            throw 'Expected ordinary untrusted-root policy result was not observed.'
        }
        return [pscustomobject]@{
            schema = 1; policy = 'development-pinned-integrity'; certificate_sha256 = $pin
            sha256 = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
            bytes = $bytes.Length; cms_signature_valid = $true; native_hash_status = '0x00000000'
            ordinary_trust_status = '0x800b0109'; publicly_trusted = $false
            timestamp = 'absent'; revocation = 'NOT_CHECKED_OFFLINE'
        }
    } finally {
        if ($null -ne $cert) { $cert.Dispose() }
        $stream.Dispose()
    }
}
