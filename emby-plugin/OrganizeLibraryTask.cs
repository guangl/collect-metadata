using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using MediaBrowser.Controller.Library;
using MediaBrowser.Model.Entities;
using MediaBrowser.Model.Logging;
using MediaBrowser.Model.Tasks;

namespace CollectMetadata.Emby;

public sealed class OrganizeLibraryTask : IScheduledTask
{
    private static readonly StringComparer PathComparer = Path.DirectorySeparatorChar == '\\'
        ? StringComparer.OrdinalIgnoreCase : StringComparer.Ordinal;
    private static readonly SemaphoreSlim Gate = new SemaphoreSlim(1, 1);
    private static Dictionary<string, MediaOrganizer.Observation> previous = new Dictionary<string, MediaOrganizer.Observation>(PathComparer);
    private readonly ILibraryManager library;
    private readonly ILibraryMonitor monitor;
    private readonly ILogger logger;

    public OrganizeLibraryTask(ILibraryManager library, ILibraryMonitor monitor, ILogManager logs)
    {
        this.library = library;
        this.monitor = monitor;
        logger = logs.GetLogger(Plugin.PluginName);
    }

    public string Name => "Organize loose library videos";
    public string Key => "CollectMetadataOrganizeLibrary";
    public string Category => Plugin.PluginName;
    public string Description => "Move stable root-level videos into work folders in explicitly enabled movie and TV libraries.";

    public IEnumerable<TaskTriggerInfo> GetDefaultTriggers() => new[]
    {
        new TaskTriggerInfo { Type = TaskTriggerInfo.TriggerInterval, IntervalTicks = TimeSpan.FromMinutes(2).Ticks },
    };

    public async Task Execute(CancellationToken cancellationToken, IProgress<double> progress)
    {
        if (!await Gate.WaitAsync(0, cancellationToken).ConfigureAwait(false)) return;
        var current = new Dictionary<string, MediaOrganizer.Observation>(PathComparer);
        try
        {
            var config = Plugin.Instance?.Configuration;
            if (config?.OrganizeEnabled == false) return;
            var value = Environment.GetEnvironmentVariable("EMBY_ORGANIZE_ROOTS") ?? config?.OrganizeRoots ?? string.Empty;
            var enabled = new HashSet<string>(SplitRoots(value).Select(NormalizeRoot).OfType<string>(), PathComparer);
            if (enabled.Count == 0) return;
            var rules = config?.Organizer ?? new OrganizerRules();
            try { rules.Validate(); }
            catch (ArgumentException error) { logger.Warn("Invalid organizer settings; no files moved: {0}", error.Message); return; }
            var roots = library.GetVirtualFolders().SelectMany(folder => (folder.Locations ?? Array.Empty<string>())
                .Select(location => (Path: NormalizeRoot(location), Folder: folder)))
                .Where(entry => entry.Path != null).ToLookup(entry => entry.Path!, entry => entry.Folder, PathComparer);
            var visited = 0;
            foreach (var root in enabled)
            {
                cancellationToken.ThrowIfCancellationRequested();
                var folders = roots[root].ToArray();
                if (folders.Length != 1 || !TryLibrary(folders[0], out var series))
                {
                    logger.Warn("Skipping organizer root {0}: expected exactly one movie/TV library with the Nfo saver and local images enabled.", root);
                }
                else
                {
                    try { Scan(root, series, roots.Select(group => group.Key), rules, current, cancellationToken); }
                    catch (Exception error) when (FileError(error))
                    { logger.Warn("Could not scan organizer root {0}: {1}", root, error.Message); }
                }
                progress.Report(100.0 * ++visited / enabled.Count);
            }
        }
        finally
        {
            previous = current;
            Gate.Release();
        }
    }

    private static bool TryLibrary(VirtualFolderInfo folder, out bool series)
    {
        series = string.Equals(folder.CollectionType, "tvshows", StringComparison.OrdinalIgnoreCase);
        var options = folder.LibraryOptions;
        return (series || string.Equals(folder.CollectionType, "movies", StringComparison.OrdinalIgnoreCase))
            && options != null && options.SaveLocalMetadata
            && (options.MetadataSavers?.Contains("Nfo", StringComparer.OrdinalIgnoreCase) ?? false);
    }

    private void Scan(string root, bool series, IEnumerable<string> libraryRoots, OrganizerRules rules,
        Dictionary<string, MediaOrganizer.Observation> current, CancellationToken cancellationToken)
    {
        if (!MediaOrganizer.PlainDirectoryChain(root)) return;
        var files = new HashSet<string>(Directory.EnumerateFiles(root), PathComparer);
        var directories = new HashSet<string>(Directory.EnumerateDirectories(root), PathComparer);
        var nestedLibraries = libraryRoots.Where(path => Below(root, path)).ToArray();
        foreach (var video in files.Where(MediaOrganizer.IsCandidate).ToArray())
        {
            cancellationToken.ThrowIfCancellationRequested();
            try
            {
                var now = DateTime.UtcNow;
                var observation = MediaOrganizer.Observe(video, now);
                if (observation == null) continue;
                if (!previous.TryGetValue(video, out var prior) || !observation.SameVersion(prior))
                {
                    current[video] = observation;
                    continue;
                }
                current[video] = prior;
                if (!observation.IsStableSince(prior, rules.QuietTime)) continue;
                var plan = MediaOrganizer.Plan(root, video, series, files.ToArray(), directories.ToArray(), rules);
                if (plan == null || nestedLibraries.Any(path => PathComparer.Equals(path, plan.Work) || Below(plan.Work, path))) continue;
                if (plan.Sources.Any(source => source != video && !MediaOrganizer.IsQuiet(source, now, rules.QuietTime))) continue;
                if (library.FindByPath(video, false, cancellationToken)?.IsLocked == true) continue;
                if (MediaOrganizer.Observe(video, DateTime.UtcNow)?.IsStableSince(prior, rules.QuietTime) != true) continue;
                plan.Move((path, error) => logger.Warn("Could not roll back organizer change at {0}: {1}", path, error.Message));
                directories.Add(plan.Work);
                foreach (var source in plan.Sources) files.Remove(source);
                current.Remove(video);
                logger.Info("Organized video {0} into {1}.", video, plan.Destination);
                // Let Emby reconcile the old and new items and run its own metadata and image savers.
                monitor.ReportFileSystemChanged(video);
                monitor.ReportFileSystemChanged(plan.Target(video));
            }
            catch (Exception error) when (FileError(error))
            { logger.Warn("Could not organize {0}: {1}", video, error.Message); }
        }
    }

    private static bool Below(string parent, string child) => child.Length > parent.Length
        && child[parent.Length] == Path.DirectorySeparatorChar && PathComparer.Equals(parent, child.Substring(0, parent.Length));

    internal static IEnumerable<string> SplitRoots(string value) => value.Split(';', '\r', '\n')
        .Select(path => path.Trim()).Where(path => path.Length != 0);

    internal static string? NormalizeRoot(string path)
    {
        path = path.Trim();
        if (!Path.IsPathRooted(path)) return null;
        if (Path.DirectorySeparatorChar == '\\' && !path.StartsWith(@"\\", StringComparison.Ordinal)
            && (path.Length < 3 || path[1] != ':' || (path[2] != '\\' && path[2] != '/'))) return null;
        try
        {
            var full = Path.GetFullPath(path).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
            return full.Length > (Path.GetPathRoot(full)?.TrimEnd(Path.DirectorySeparatorChar).Length ?? 0) ? full : null;
        }
        catch (Exception error) when (error is ArgumentException || error is NotSupportedException || FileError(error))
        { return null; }
    }

    private static bool FileError(Exception error) => error is IOException || error is UnauthorizedAccessException;
}
