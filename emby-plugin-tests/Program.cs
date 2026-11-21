using System;
using System.IO;
using System.Linq;
using CollectMetadata.Emby;

// Use the real temporary path on macOS, where /var is a symlink.
var scratch = Path.GetTempPath();
if (OperatingSystem.IsMacOS() && scratch.StartsWith("/var/", StringComparison.Ordinal)) scratch = "/private" + scratch;
var root = Directory.CreateDirectory(Path.Combine(scratch, "emby-organizer-" + Guid.NewGuid().ToString("N"))).FullName;
try
{
    Movies();
    Episodes();
    ConflictsAndLinks();
    Templates();
    Stability();
    Console.WriteLine("All media organizer checks passed.");
}
finally { Directory.Delete(root, recursive: true); }

void Movies()
{
    var movies = MakeDir(root, ".private-video");
    var hiddenVideo = Put(movies, ".hidden.mkv");
    Check(Plan(movies, hiddenVideo, series: false) == null, "a hidden input video was planned");
    var video = Put(movies, "电影 (2025).mkv");
    Put(movies, "电影 (2025).nfo");
    Put(movies, "电影 (2025).zh-CN.forced.srt");
    Put(movies, "电影 (2025)-poster.jpg");
    Put(movies, "poster.jpg");
    Put(movies, "Another Movie.en.mkv");
    Put(movies, "Another Movie.en.srt");
    var similar = Put(movies, "Another Movie.mkv");
    Check(Plan(movies, similar, series: false)?.Sources.Length == 1, "subtitle assigned to the wrong video");
    var plan = Plan(movies, video, series: false)!;
    Check(plan.Sources.Length == 4, "movie sidecar selection");
    plan.Move(UnexpectedRollback);
    Check(!File.Exists(video) && File.ReadAllText(plan.Target(video)) == "test", "movie wasn't moved intact");
    Check(File.Exists(Path.Combine(plan.Work, "电影 (2025).nfo")), "NFO didn't follow movie");
    Check(File.Exists(Path.Combine(plan.Work, "电影 (2025).zh-CN.forced.srt")), "subtitle didn't follow movie");
    Check(File.Exists(Path.Combine(plan.Work, "电影 (2025)-poster.jpg")), "image didn't follow movie");
    Check(File.Exists(Path.Combine(movies, "poster.jpg")), "ambiguous poster was moved");
    Check(Plan(movies, plan.Target(video), series: false) == null, "a nested video was planned");
    var duplicate = Put(movies, "电影 (2025) [tmdbid=123].mp4");
    Check(Plan(movies, duplicate, series: false) == null, "a duplicate work folder was planned");
    var multi = Put(movies, "Other.mkv");
    Put(movies, "Other.mp4");
    Check(Plan(movies, multi, series: false) == null, "ambiguous root versions were treated as a single movie");
}

void Episodes()
{
    var tv = MakeDir(root, "番剧");
    var show = MakeDir(tv, "剧名 (2024) [tmdbid=123]");
    var season = MakeDir(show, "Season 01");
    var existingEpisode = Put(season, "剧名 (2024) S01E01.mkv");
    Put(show, "tvshow.nfo");
    var video = Put(tv, "剧名 S01E02.mkv");
    var plan = Plan(tv, video, series: true)!;
    Check(plan.Destination == season, "existing season was not reused");
    plan.Move(UnexpectedRollback);
    Check(File.Exists(plan.Target(video)) && File.Exists(existingEpisode) && File.Exists(Path.Combine(show, "tvshow.nfo")), "existing season files changed");
    var newShow = Put(tv, "A.Show.S01E01.1080p.mp4");
    plan = Plan(tv, newShow, series: true)!;
    Check(plan.Destination == Path.Combine(tv, "A Show", "Season 1"), "new season layout differs from Emby library");
    plan.Move(UnexpectedRollback);
    var next = Put(tv, "A.Show.1x02.mp4");
    Check(Plan(tv, next, series: true)?.Destination == plan.Destination, "second episode didn't use the same directory");
    Check(Plan(tv, Put(tv, "Unknown 01.mp4"), series: true) == null, "guessed an unmarked episode");
    Check(Plan(tv, Put(tv, "剧名 (2024) [tmdbid=999] S01E03.mkv"), series: true) == null, "episodes from different show IDs were merged");
    MakeDir(tv, "Ambiguous [tmdbid=1]");
    MakeDir(tv, "Ambiguous [tmdbid=2]");
    Check(Plan(tv, Put(tv, "Ambiguous S01E01.mp4"), series: true) == null, "an ambiguous series ID was chosen");
    MakeDir(tv, "Remake (2004) [tmdbid=3]");
    MakeDir(tv, "Remake (2024) [tmdbid=4]");
    Check(Plan(tv, Put(tv, "Remake S01E01.mp4"), series: true) == null, "an ambiguous release year was chosen");
    var explicitYear = Put(tv, "Remake (2024) S01E01.mp4");
    Check(Plan(tv, explicitYear, series: true)?.Work == Path.Combine(tv, "Remake (2024) [tmdbid=4]"), "explicit release year wasn't respected");
}

void ConflictsAndLinks()
{
    var tv = MakeDir(root, "Conflict");
    var work = MakeDir(tv, "Show");
    var existing = MakeDir(work, "Season 1");
    var video = Put(tv, "Show S01E01.mkv");
    var nfo = Put(tv, "Show S01E01.nfo");
    var plan = Plan(tv, video, series: true)!;
    var collision = Put(existing, Path.GetFileName(nfo), "existing");
    try { plan.Move(UnexpectedRollback); throw new Exception("collision did not fail"); }
    catch (IOException) { }
    Check(File.Exists(video) && File.Exists(nfo) && File.ReadAllText(collision) == "existing", "collision lost or overwrote files");
    if (!OperatingSystem.IsWindows())
    {
        var outside = MakeDir(root, "Outside");
        Directory.CreateSymbolicLink(Path.Combine(tv, "Linked"), outside);
        var linkVideo = Put(tv, "Linked S01E01.mkv");
        Check(Plan(tv, linkVideo, series: true) == null, "planned a symlink destination");
        var linkFile = Path.Combine(tv, "Linked File.mkv");
        File.CreateSymbolicLink(linkFile, video);
        Check(MediaOrganizer.Observe(linkFile, DateTime.UtcNow) == null, "accepted a symlink video");
        Check(!Directory.EnumerateFileSystemEntries(outside).Any(), "wrote outside the media root");
    }
}

void Templates()
{
    var rules = new OrganizerRules
    {
        MovieFolder = "{title}{year_suffix} - ID {tmdbid}", MovieFile = "{title}{year_suffix}",
        SeriesFolder = "{title}{year_suffix}{tmdb_suffix}", SeasonFolder = "第 {season:00} 季",
        EpisodeFile = "{title} - S{season:00}E{episode:00}", MoveImages = false,
    };
    new OrganizerRules().Validate();
    rules.Validate();
    var movies = MakeDir(root, "Templates");
    var movie = Put(movies, "电影 (2025) [tmdbid=12].MKV");
    var nfo = Put(movies, "电影 (2025) [tmdbid=12].nfo");
    var subtitle = Put(movies, "电影 (2025) [tmdbid=12].zh-CN.srt");
    var poster = Put(movies, "电影 (2025) [tmdbid=12]-poster.jpg");
    var plan = Plan(movies, movie, false, rules)!;
    Check(plan.Work == Path.Combine(movies, "电影 (2025) - ID 12"), "directory template ignored metadata tokens");
    Check(Path.GetFileName(plan.Target(movie)) == "电影 (2025).MKV", "extension was changed");
    Check(Path.GetFileName(plan.Target(nfo)) == "电影 (2025).nfo" && Path.GetFileName(plan.Target(subtitle)) == "电影 (2025).zh-CN.srt", "sidecars not renamed with video");
    plan.Move(UnexpectedRollback);
    Check(File.Exists(poster) && File.Exists(plan.Target(nfo)), "move image switch or NFO rename wasn't applied");
    var tv = MakeDir(root, "TV Templates");
    var episode = Put(tv, "剧名 (2024) S02E03.1080p.MKV");
    plan = Plan(tv, episode, true, rules)!;
    Check(plan.Target(episode) == Path.Combine(tv, "剧名 (2024)", "第 02 季", "剧名 - S02E03.MKV"), "episode template generated an unexpected path");
    var multi = Put(tv, "剧名 (2024) S02E03-E04.1080p.MKV");
    Check(Path.GetFileName(Plan(tv, multi, true, rules)!.Target(multi)) == Path.GetFileName(multi), "multi-episode rename lost the range");
    rules.EpisodeFile = "{title} - {episode_code}";
    rules.Validate();
    Check(Path.GetFileName(Plan(tv, multi, true, rules)!.Target(multi)) == "剧名 - S02E03-E04.MKV", "episode code didn't retain the full range");
    rules.EpisodeFile = "[${subgroup}] ${title} S${seasonFormat}E${episodeFormat} ${resolution}";
    rules.Validate();
    var ani = Put(tv, "[字幕组] 番剧 (2025) S01E07 [1080p].mkv");
    var aniWork = MakeDir(tv, "番剧 (2025) [tmdbid=88]");
    var aniSeason = MakeDir(aniWork, "Season 01");
    var aniPlan = Plan(tv, ani, true, rules)!;
    Check(Path.GetFileName(aniPlan.Target(ani)) == "[字幕组] 番剧 S01E07 1080p.mkv", "ani-rss variables did not render");
    Check(aniPlan.Destination == aniSeason, "release group caused a duplicate series or season folder");
    Check(Path.GetFileName(Plan(tv, episode, true, rules)!.Target(episode)) == "剧名 S02E03 1080p.MKV", "empty subtitle group left empty brackets");
    rules.RenameEpisodes = false;
    Check(Path.GetFileName(Plan(tv, ani, true, rules)!.Target(ani)) == Path.GetFileName(ani), "rename switch did not preserve the original");
    rules.RenameMovies = true;
    rules.RemoveYearFromFiles = rules.RemoveTmdbIdFromFiles = true;
    rules.MovieFolder = "{original}";
    rules.MovieFile = "${itemTitle}";
    var identity = Put(movies, "另一部电影 (2025) [tmdbid=99].mp4");
    Check(Path.GetFileName(Plan(movies, identity, false, rules)!.Target(identity)) == "另一部电影.mp4", "filename-only removal options did not apply");
    rules.MovieFolder = "../{original}";
    RejectRules(rules);
    rules.MovieFolder = "{typo}";
    RejectRules(rules);
    rules.MovieFolder = "{title}";
    rules.EpisodeFile = "{title} - {episode}";
    RejectRules(rules);
}

void Stability()
{
    var now = DateTime.UtcNow;
    var wait = TimeSpan.FromMinutes(2);
    var old = new MediaOrganizer.Observation(10, now.AddMinutes(-3), now);
    Check(!new MediaOrganizer.Observation(10, now.AddMinutes(-3), now.AddSeconds(119)).IsStableSince(old, wait), "accepted early copy");
    Check(new MediaOrganizer.Observation(10, now.AddMinutes(-3), now.AddMinutes(2)).IsStableSince(old, wait), "stable file wasn't accepted");
    Check(!new MediaOrganizer.Observation(11, now.AddMinutes(-3), now.AddMinutes(2)).IsStableSince(old, wait), "accepted a growing file");
}

static void RejectRules(OrganizerRules rules)
{
    try { rules.Validate(); }
    catch (ArgumentException) { return; }
    throw new Exception("invalid naming rule was accepted");
}

static MediaOrganizer.MovePlan? Plan(string root, string video, bool series, OrganizerRules? rules = null) =>
    MediaOrganizer.Plan(root, video, series, Directory.GetFiles(root), Directory.GetDirectories(root), rules);
static string MakeDir(string dir, string name) => Directory.CreateDirectory(Path.Combine(dir, name)).FullName;
static string Put(string dir, string name, string contents = "test")
{
    var path = Path.Combine(dir, name);
    File.WriteAllText(path, contents);
    return path;
}
static void UnexpectedRollback(string path, Exception cause) => throw new Exception(path, cause);
static void Check(bool check, string message) { if (!check) throw new Exception(message); }
