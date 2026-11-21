using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text.RegularExpressions;

namespace CollectMetadata.Emby;

internal static class MediaOrganizer
{
    private static readonly HashSet<string> VideoExtensions = new HashSet<string>(StringComparer.OrdinalIgnoreCase)
        { ".mkv", ".mp4", ".avi", ".mov", ".m4v", ".wmv", ".ts", ".m2ts", ".webm", ".mpg", ".mpeg" };
    private static readonly HashSet<string> SubtitleExtensions = new HashSet<string>(StringComparer.OrdinalIgnoreCase)
        { ".srt", ".ass", ".ssa", ".vtt", ".sub", ".idx", ".sup" };
    private static readonly HashSet<string> ImageExtensions = new HashSet<string>(StringComparer.OrdinalIgnoreCase)
        { ".jpg", ".jpeg", ".png", ".webp" };
    private static readonly HashSet<string> ImageSuffixes = new HashSet<string>(StringComparer.OrdinalIgnoreCase)
        { "", "-poster", "-fanart", "-thumb", "-backdrop", "-banner", "-logo" };
    private static readonly Regex Episode = new Regex(
        @"\A(?<series>.+?)[ ._\-]+(?<code>S(?<season>[0-9]{1,3})[ ._\-]*E(?<episode>[0-9]{1,4})(?<multi>(?:[ ._\-]*E[0-9]{1,4}|-[0-9]{1,4}(?=$|[^\p{L}\p{N}]))+)?|(?<season>[0-9]{1,3})x(?<episode>[0-9]{1,4})(?<multi>-(?:[0-9]{1,3}x)?[0-9]{1,4})?)(?=$|[^\p{L}\p{N}])",
        RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);
    private static readonly Regex TmdbTag = new Regex(@"\s*\[tmdbid[=-](?<id>[0-9]+)\]\s*\z", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);
    private static readonly Regex YearTag = new Regex(@"\s+\((?<year>(?:19|20)[0-9]{2})\)\s*\z", RegexOptions.CultureInvariant);
    private static readonly Regex ReleaseGroup = new Regex(@"\A(?:\[(?<group>[^\[\]]+)\]|【(?<group>[^【】]+)】)\s*", RegexOptions.CultureInvariant);
    private static readonly Regex Resolution = new Regex(@"(?<![a-z0-9])(?:2160p|1440p|1080p|1080i|720p|576p|480p|4k)(?![a-z0-9])", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);
    private static readonly Regex YearInName = new Regex(@"\s*\((?:19|20)[0-9]{2}\)", RegexOptions.CultureInvariant);
    private static readonly Regex TmdbInName = new Regex(@"\s*\[tmdbid[=-][0-9]+\]", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);
    private static readonly Regex SeasonFolder = new Regex(@"\ASeason\s+0*(?<season>[0-9]{1,3})\z", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);
    private static readonly Regex SkipStem = new Regex(@"(?:\A|[ ._\-])(?:sample|trailer|partial|part|tmp|cd[0-9]+|disc[0-9]+)\z", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);
    private static readonly Regex SubtitleTags = new Regex(
        @"\A\.(?:(?:[a-z]{2,3}(?:[-_][a-z0-9]{2,8})*|default|forced|sdh|cc|hi)(?:\.|\z))+\z",
        RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);

    internal static bool IsVideoName(string value) => VideoExtensions.Contains(Path.GetExtension(value));

    internal static bool IsCandidate(string path)
    {
        var stem = Path.GetFileNameWithoutExtension(path);
        return VideoExtensions.Contains(Path.GetExtension(path)) && UsableName(stem) && !SkipStem.IsMatch(stem);
    }

    private static bool UsableName(string name) => !string.IsNullOrWhiteSpace(name)
        && name[0] != '.' && name[0] != '@' && name != ".." && name.IndexOfAny(Path.GetInvalidFileNameChars()) < 0;

    internal static bool PlainEntry(string path, bool directory)
    {
        var attributes = File.GetAttributes(path);
        return (attributes & (FileAttributes.ReparsePoint | FileAttributes.Hidden | FileAttributes.System)) == 0
            && ((attributes & FileAttributes.Directory) != 0) == directory;
    }

    internal static bool PlainDirectoryChain(string path)
    {
        // A root explicitly configured in Emby can be hidden (for example .private-video).
        // Still prohibit symlinks in the whole path and keep ignoring hidden files and work folders.
        for (var current = new DirectoryInfo(path); current != null; current = current.Parent)
        {
            var attributes = File.GetAttributes(current.FullName);
            if ((attributes & (FileAttributes.ReparsePoint | FileAttributes.Directory)) != FileAttributes.Directory) return false;
        }
        return true;
    }

    private static readonly OrganizerRules Defaults = new OrganizerRules();

    internal sealed class Observation
    {
        private readonly long size;
        private readonly DateTime lastWrite;
        private readonly DateTime seen;

        internal Observation(long size, DateTime lastWrite, DateTime seen)
        {
            this.size = size;
            this.lastWrite = lastWrite;
            this.seen = seen;
        }

        internal bool SameVersion(Observation earlier) => size == earlier.size && lastWrite == earlier.lastWrite;

        internal bool IsStableSince(Observation earlier, TimeSpan quietTime) => SameVersion(earlier)
            && seen - earlier.seen >= quietTime && seen - lastWrite >= quietTime;
    }

    internal static Observation? Observe(string path, DateTime now)
    {
        if (!PlainEntry(path, directory: false)) return null;
        var file = new FileInfo(path);
        return file.Length > 0 ? new Observation(file.Length, file.LastWriteTimeUtc, now) : null;
    }

    internal static bool IsQuiet(string path, DateTime now, TimeSpan quietTime) => PlainEntry(path, directory: false)
        && now - File.GetLastWriteTimeUtc(path) >= quietTime;

    internal static bool ValidEpisodeTemplate(string template)
    {
        const string sample = "Sample (2025) S02E03.1080p";
        var episode = Episode.Match(sample);
        return PreservesEpisode(OrganizerRules.Render(template, NamingValues(sample, "Sample (2025)", episode)), episode);
    }

    private static Dictionary<string, string> NamingValues(string original, string name, Match episode)
    {
        var tmdb = TmdbTag.Match(name).Groups["id"].Value;
        var withoutId = TmdbTag.Replace(name, "");
        var year = YearTag.Match(withoutId).Groups["year"].Value;
        var cleanName = YearTag.Replace(withoutId, "");
        var group = ReleaseGroup.Match(cleanName).Groups["group"].Value;
        var title = ReleaseGroup.Replace(cleanName, "").Trim().Replace('_', ' ');
        var values = new Dictionary<string, string>(StringComparer.Ordinal)
        {
            ["original"] = original, ["work"] = name, ["title"] = title, ["year"] = year,
            ["year_suffix"] = year.Length == 0 ? "" : " (" + year + ")", ["tmdbid"] = tmdb,
            ["tmdb_suffix"] = tmdb.Length == 0 ? "" : " [tmdbid=" + tmdb + "]",
            ["subgroup"] = group, ["resolution"] = Resolution.Match(original).Value,
        };
        if (episode.Success)
        {
            OrganizerRules.AddNumber(values, "season", Number(episode, "season"));
            OrganizerRules.AddNumber(values, "episode", Number(episode, "episode"));
            values["episode_code"] = episode.Groups["code"].Value;
        }
        return values;
    }

    private static int Number(Match match, string group) => int.Parse(match.Groups[group].Value, System.Globalization.CultureInfo.InvariantCulture);

    private static bool PreservesEpisode(string rendered, Match input)
    {
        var output = Episode.Match("Title " + rendered);
        return output.Success && Number(output, "season") == Number(input, "season") && Number(output, "episode") == Number(input, "episode")
            && (input.Groups["multi"].Success || output.Groups["multi"].Success
                ? Same(input.Groups["code"].Value, output.Groups["code"].Value) : true);
    }

    // Only root-level files are supplied. A movie directory is never merged into or overwritten.
    // For TV, reuse only a single unambiguous series/season directory.
    internal static MovePlan? Plan(string root, string video, bool series, string[] files, string[] directories, OrganizerRules? rules = null)
    {
        rules ??= Defaults;
        if (Path.GetDirectoryName(video) != root || !IsCandidate(video)) return null;
        var stem = Path.GetFileNameWithoutExtension(video);
        if (files.Count(file => VideoExtensions.Contains(Path.GetExtension(file))
            && Same(Path.GetFileNameWithoutExtension(file), stem)) != 1) return null;
        var episode = Episode.Match(stem);
        if (episode.Success != series) return null;

        var name = series ? episode.Groups["series"].Value.TrimEnd(' ', '.', '-', '_').Replace('.', ' ').Replace('_', ' ') : stem;
        if (!UsableName(name)) return null;
        var values = NamingValues(stem, name, episode);
        var folder = OrganizerRules.Render(series ? rules.SeriesFolder : rules.MovieFolder, values);
        var renameEnabled = series ? rules.RenameEpisodes : rules.RenameMovies;
        var fileTemplate = series ? rules.EpisodeFile : rules.MovieFile;
        var renamed = renameEnabled && OrganizerRules.Normalize(fileTemplate) != "{original}" ? OrganizerRules.Render(fileTemplate, values) : stem;
        if (renameEnabled && rules.RemoveYearFromFiles) renamed = YearInName.Replace(renamed, "").Trim();
        if (renameEnabled && rules.RemoveTmdbIdFromFiles) renamed = TmdbInName.Replace(renamed, "").Trim();
        // A fractional release marker is not interchangeable with its integer episode.
        if (series && Regex.IsMatch(stem.Substring(episode.Length), @"\A\.5(?=$|[^\p{L}\p{N}])")) renamed = stem;
        // A collected range such as S01E01-E02 must not turn into a single episode.
        if (series && !PreservesEpisode(renamed, episode)) renamed = stem;
        if (!OrganizerRules.UsableSegment(folder) || !OrganizerRules.UsableSegment(renamed + Path.GetExtension(video))) return null;
        var exact = directories.Where(dir => Same(Path.GetFileName(dir), folder)).ToArray();
        var baseName = ReleaseGroup.Replace(TmdbTag.Replace(name, ""), "").Trim();
        var tmdb = TmdbTag.Match(name).Groups["id"].Value;
        var noIdentity = tmdb.Length == 0 && !YearTag.IsMatch(baseName);
        var related = directories.Where(dir =>
        {
            var existing = ReleaseGroup.Replace(TmdbTag.Replace(Path.GetFileName(dir), ""), "").Trim();
            return Same(existing, baseName) || (noIdentity && Same(YearTag.Replace(existing, ""), baseName));
        }).ToArray();
        var candidates = exact.Concat(related).Distinct().ToArray();
        if (candidates.Length > 1 || (!series && candidates.Length != 0)) return null;
        var reuse = series ? candidates.SingleOrDefault() : null;
        if (reuse != null && tmdb.Length != 0)
        {
            var existing = Path.GetFileName(reuse);
            var existingId = TmdbTag.Match(existing).Groups["id"].Value;
            if (existingId != tmdb && (existingId.Length != 0 || !Same(existing, folder))) return null;
        }
        if (reuse != null && !PlainEntry(reuse, directory: true)) return null;
        var work = reuse ?? Path.Combine(root, folder);
        var destination = work;
        if (series)
        {
            var season = Number(episode, "season");
            var seasonFolder = OrganizerRules.Render(rules.SeasonFolder, values);
            if (!OrganizerRules.UsableSegment(seasonFolder)) return null;
            var matches = reuse == null ? Array.Empty<string>() : Directory.EnumerateDirectories(work)
                .Where(dir => Same(Path.GetFileName(dir), seasonFolder) || SeasonNumber(Path.GetFileName(dir)) == season).Take(2).ToArray();
            if (matches.Length > 1 || (matches.Length == 1 && !PlainEntry(matches[0], directory: true))) return null;
            destination = matches.SingleOrDefault() ?? Path.Combine(work, seasonFolder);
        }
        var otherStems = files.Where(file => file != video && VideoExtensions.Contains(Path.GetExtension(file)))
            .Select(file => Path.GetFileNameWithoutExtension(file)).Where(other => other.Length > stem.Length).ToArray();
        var sources = files.Where(file => MovesSidecar(file, rules) && IsSidecar(stem, file) && !otherStems.Any(other => IsSidecar(other, file)))
            .Append(video).ToArray();
        var plan = new MovePlan(root, work, destination, sources, reuse != null, stem, renamed);
        return sources.All(source => OrganizerRules.UsableSegment(Path.GetFileName(plan.Target(source)))) ? plan : null;
    }

    private static int SeasonNumber(string name)
    {
        var season = SeasonFolder.Match(name);
        return season.Success ? int.Parse(season.Groups["season"].Value, System.Globalization.CultureInfo.InvariantCulture) : -1;
    }

    private static bool MovesSidecar(string path, OrganizerRules rules)
    {
        var extension = Path.GetExtension(path);
        return rules.MoveNfo && Same(extension, ".nfo") || rules.MoveSubtitles && SubtitleExtensions.Contains(extension)
            || rules.MoveImages && ImageExtensions.Contains(extension);
    }

    private static bool IsSidecar(string stem, string path)
    {
        var name = Path.GetFileNameWithoutExtension(path);
        var extension = Path.GetExtension(path);
        if (extension.Equals(".nfo", StringComparison.OrdinalIgnoreCase)) return Same(name, stem);
        if (!name.StartsWith(stem, StringComparison.OrdinalIgnoreCase)) return false;
        var suffix = name.Substring(stem.Length);
        if (SubtitleExtensions.Contains(extension)) return suffix.Length == 0 || SubtitleTags.IsMatch(suffix);
        return ImageExtensions.Contains(extension) && ImageSuffixes.Contains(suffix);
    }

    private static bool Same(string first, string second) => first.Equals(second, StringComparison.OrdinalIgnoreCase);

    internal sealed class MovePlan
    {
        internal readonly string Root;
        internal readonly string Work;
        internal readonly string Destination;
        internal readonly string[] Sources;

        private readonly bool reuseWork;
        private readonly string originalStem;
        private readonly string renamedStem;

        internal MovePlan(string root, string work, string destination, string[] sources, bool reuseWork, string originalStem, string renamedStem)
        {
            Root = root;
            Work = work;
            Destination = destination;
            Sources = sources; // Video last: an incomplete move never publishes a playable entry first.
            this.reuseWork = reuseWork;
            this.originalStem = originalStem;
            this.renamedStem = renamedStem;
        }

        internal string Target(string source)
        {
            var name = Path.GetFileName(source);
            return Path.Combine(Destination, originalStem == renamedStem ? name : renamedStem + name.Substring(originalStem.Length));
        }

        internal void Move(Action<string, Exception> rollbackError)
        {
            var created = new List<string>();
            var moved = new List<string>();
            try
            {
                if (!PlainDirectoryChain(Root)) throw new IOException("The library path is not a regular directory.");
                foreach (var source in Sources)
                {
                    if (!PlainEntry(source, directory: false)) throw new IOException("The source is not a regular file.");
                    // Best effort: in addition to stability checks, respect open handles that deny sharing.
                    using (File.Open(source, FileMode.Open, FileAccess.Read, FileShare.None)) { }
                }
                if (!reuseWork && Directory.Exists(Work)) throw new IOException("The work directory already exists.");
                EnsureDirectory(Work, created);
                if (Destination != Work) EnsureDirectory(Destination, created);
                var occupied = new HashSet<string>(Directory.EnumerateFileSystemEntries(Destination).Select(path => Path.GetFileName(path)!),
                    StringComparer.OrdinalIgnoreCase);
                foreach (var source in Sources)
                {
                    if (!occupied.Add(Path.GetFileName(Target(source)))) throw new IOException("A destination with that file name already exists.");
                }
                foreach (var source in Sources)
                {
                    File.Move(source, Target(source)); // Never overwrite existing files.
                    moved.Add(source);
                }
            }
            catch
            {
                for (var index = moved.Count - 1; index >= 0; index--)
                {
                    var source = moved[index];
                    try { File.Move(Target(source), source); }
                    catch (Exception error) when (error is IOException || error is UnauthorizedAccessException)
                    { rollbackError(Target(source), error); }
                }
                for (var index = created.Count - 1; index >= 0; index--)
                {
                    try { Directory.Delete(created[index], recursive: false); }
                    catch (Exception error) when (error is IOException || error is UnauthorizedAccessException)
                    { rollbackError(created[index], error); }
                }
                throw;
            }
        }

        private static void EnsureDirectory(string path, List<string> created)
        {
            if (!Directory.Exists(path))
            {
                Directory.CreateDirectory(path);
                created.Add(path);
            }
            if (!PlainEntry(path, directory: true)) throw new IOException("The target is not a regular directory.");
        }
    }
}
