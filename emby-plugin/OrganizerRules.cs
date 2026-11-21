using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Text;
using System.Text.RegularExpressions;

namespace CollectMetadata.Emby;

public sealed class OrganizerRules
{
    public string MovieFolder { get; set; } = "{original}";
    public string SeriesFolder { get; set; } = "{work}";
    public string SeasonFolder { get; set; } = "Season {season}";
    public string MovieFile { get; set; } = "{original}";
    public string EpisodeFile { get; set; } = "{original}";
    public int StableSeconds { get; set; } = 60;
    public bool MoveNfo { get; set; } = true;
    public bool MoveSubtitles { get; set; } = true;
    public bool MoveImages { get; set; } = true;
    public bool RenameMovies { get; set; } = true;
    public bool RenameEpisodes { get; set; } = true;
    public bool RemoveYearFromFiles { get; set; }
    public bool RemoveTmdbIdFromFiles { get; set; }

    internal TimeSpan QuietTime => TimeSpan.FromSeconds(StableSeconds);

    private const string Identity = "original work title year year_suffix tmdbid tmdb_suffix";
    private const string Episode = "season season:00 episode episode:00 episode_code";
    private const string FileDetails = " subgroup resolution";
    private static readonly Regex Token = new Regex(@"\{([a-z0-9_:]+)\}", RegexOptions.CultureInvariant);
    private static readonly Regex ReservedName = new Regex(@"\A(?:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\.|\z)", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);
    private static readonly char[] Separators = { '/', '\\' };

    internal void Validate()
    {
        if (StableSeconds < 60 || StableSeconds > 3600)
            throw new ArgumentException("文件稳定等待时间必须在 60 到 3600 秒之间。");
        Check(MovieFolder, Identity, "电影目录", "original", "work", "title");
        Check(SeriesFolder, "work title year year_suffix tmdbid tmdb_suffix", "剧集目录", "work", "title");
        Check(SeasonFolder, "season season:00", "季目录", "season", "season:00");
        Check(MovieFile, Identity + FileDetails, "电影文件名");
        Check(EpisodeFile, Identity + FileDetails + " " + Episode, "分集文件名", "original", "episode_code", "episode", "episode:00");
        if (MediaOrganizer.IsVideoName(MovieFile.Trim()) || MediaOrganizer.IsVideoName(EpisodeFile.Trim()))
            throw new ArgumentException("视频文件名模板不需要扩展名，原扩展名会自动保留。");
        if (!MediaOrganizer.ValidEpisodeTemplate(EpisodeFile))
            throw new ArgumentException("分集文件名必须保留可识别的季集号，例如 {title} - {episode_code}。");
    }

    private static void Check(string template, string allowed, string label, params string[] required)
    {
        if (string.IsNullOrWhiteSpace(template) || template.Length > 200 || template.IndexOfAny(Separators) >= 0
            || ContainsControl(template))
            throw new ArgumentException(label + "模板无效：只允许单个目录名或不带扩展名的文件名。");
        template = Normalize(template);
        var names = new HashSet<string>(allowed.Split(' '), StringComparer.Ordinal);
        var keys = new HashSet<string>(StringComparer.Ordinal);
        foreach (Match entry in Token.Matches(template))
        {
            var key = entry.Groups[1].Value;
            if (!names.Contains(key)) throw new ArgumentException(label + "模板不支持 {" + key + "}。");
            keys.Add(key);
        }
        var literal = Token.Replace(template, string.Empty);
        if (literal.IndexOfAny(new[] { '{', '}' }) >= 0 || literal.IndexOfAny(Path.GetInvalidFileNameChars()) >= 0
            || !UsableSegment(Token.Replace(template, "示例").Trim().TrimEnd('.')))
            throw new ArgumentException(label + "模板包含无效字符或名称。");
        if (required.Length > 0 && !Array.Exists(required, keys.Contains))
            throw new ArgumentException(label + "模板至少需要一个标记：{" + string.Join("}、{", required) + "}。");
    }

    internal static string Normalize(string template) => template.Replace("${seasonFormat}", "{season:00}")
        .Replace("${episodeFormat}", "{episode:00}").Replace("${itemTitle}", "{original}").Replace("${", "{");

    internal static string Render(string template, IReadOnlyDictionary<string, string> values)
    {
        template = Normalize(template);
        if (!values.TryGetValue("subgroup", out var subgroup) || subgroup.Length == 0)
            template = template.Replace("[{subgroup}]", "").Replace("【{subgroup}】", "");
        return Token.Replace(template, match => values.TryGetValue(match.Groups[1].Value, out var value) ? value : string.Empty)
            .Trim().TrimEnd('.', ' ');
    }

    internal static bool UsableSegment(string name) => !string.IsNullOrWhiteSpace(name)
        && name[0] != '.' && name[0] != '@' && name.IndexOfAny(Separators) < 0
        && name.IndexOfAny(Path.GetInvalidFileNameChars()) < 0 && !ContainsControl(name)
        && !ReservedName.IsMatch(name) && Encoding.UTF8.GetByteCount(name) <= 240;

    private static bool ContainsControl(string value) => Array.Exists(value.ToCharArray(), char.IsControl);

    internal static void AddNumber(IDictionary<string, string> values, string key, int number)
    {
        values[key] = number.ToString(CultureInfo.InvariantCulture);
        values[key + ":00"] = number.ToString("00", CultureInfo.InvariantCulture);
    }
}
