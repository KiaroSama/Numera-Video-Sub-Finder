using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;

// All filesystem work lives in the killable observer, never in the deadline/pipe owner.
public sealed class NvsfCiFileProgress : IDisposable {
    readonly string root;
    readonly Stopwatch clock = Stopwatch.StartNew();
    Dictionary<string, (long size, long written)> previous = new(StringComparer.Ordinal);
    Dictionary<string, (long size, long written)> current = new(StringComparer.Ordinal);
    IEnumerator<FileSystemInfo> ports;
    IEnumerator<FileSystemInfo> files;
    bool baseline = true;
    int directories;
    double lastSample = -250;
    public NvsfCiFileProgress(string path, string checkout) {
        root = path;
        var cursor = new DirectoryInfo(root);
        while (cursor != null && cursor.FullName != checkout) {
            if (cursor.Exists) Attributes(cursor);
            cursor = cursor.Parent;
        }
        if (cursor == null) throw new InvalidOperationException("Progress root escaped checkout.");
        Directory.CreateDirectory(root);
        try { BeginCycle(); } catch { Dispose(); throw; }
    }
    static FileAttributes Attributes(FileSystemInfo entry) {
        var attributes = entry.Attributes;
        if ((attributes & FileAttributes.ReparsePoint) != 0)
            throw new InvalidOperationException("Owned build log entry may not be linked.");
        return attributes;
    }
    void SelectDirectory(DirectoryInfo directory) {
        if (!directory.Exists || (Attributes(directory) & FileAttributes.Directory) == 0)
            throw new InvalidOperationException("Owned build log directory is missing or invalid.");
        files = directory.EnumerateFileSystemInfos().GetEnumerator();
    }
    void BeginCycle() {
        var directory = new DirectoryInfo(root);
        directories = 1;
        SelectDirectory(directory);
        ports = directory.EnumerateFileSystemInfos().GetEnumerator();
    }
    public bool Initialized => !baseline;
    public bool Consume() {
        if (clock.Elapsed.TotalMilliseconds - lastSample < 250) return false;
        var page = Stopwatch.StartNew();
        bool changed = false;
        // Raw entries count, including source noise; do not hide scans behind a glob.
        for (int entries = 0; entries < 128 && page.ElapsedMilliseconds < 100; entries++) {
            if (files != null) {
                if (!files.MoveNext()) { files.Dispose(); files = null; continue; }
                var entry = files.Current;
                var attributes = Attributes(entry);
                if ((attributes & FileAttributes.Directory) != 0
                    || !entry.Name.EndsWith(".log", StringComparison.OrdinalIgnoreCase)) continue;
                if (entry is not FileInfo file)
                    throw new InvalidOperationException("Owned build log entry is not a regular file.");
                if (current.Count >= 8192)
                    throw new InvalidOperationException("Owned build log file count exceeded maximum=8192.");
                var value = (file.Length, file.LastWriteTimeUtc.Ticks);
                current.Add(file.FullName, value);
                if (!baseline)
                    changed |= previous.TryGetValue(file.FullName, out var old) ? old != value : value.Item1 > 0;
            } else if (ports.MoveNext()) {
                var entry = ports.Current;
                if ((Attributes(entry) & FileAttributes.Directory) == 0) continue;
                if (entry is not DirectoryInfo port)
                    throw new InvalidOperationException("Owned build log port is not a directory.");
                if (++directories > 513)
                    throw new InvalidOperationException("Owned build log directory count exceeded maximum=513.");
                SelectDirectory(port);
            } else {
                ports.Dispose(); ports = null;
                previous = current;
                current = new(StringComparer.Ordinal);
                baseline = false;
                BeginCycle();
                break;
            }
        }
        lastSample = clock.Elapsed.TotalMilliseconds;
        return changed;
    }
    public void Dispose() { files?.Dispose(); ports?.Dispose(); }
}
