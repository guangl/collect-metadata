using System;
using System.Collections.Generic;
using System.Net;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;
using System.Threading.Tasks;
using MediaBrowser.Common;
using MediaBrowser.Controller.Dto;
using MediaBrowser.Controller.Entities;
using MediaBrowser.Controller.Library;
using MediaBrowser.Model.Logging;
using MediaBrowser.Model.Querying;
using MediaBrowser.Model.Serialization;
using MediaBrowser.Model.Tasks;

namespace CollectMetadata.Emby;

public sealed class SyncLibraryTask : IScheduledTask
{
    private static readonly HttpClient Http = new HttpClient(new HttpClientHandler { AllowAutoRedirect = false })
    {
        Timeout = TimeSpan.FromSeconds(30),
    };

    private readonly ILibraryManager library;
    private readonly IDtoService dto;
    private readonly IJsonSerializer json;
    private readonly ILogger logger;
    private readonly string serverId;

    public SyncLibraryTask(ILibraryManager library, IDtoService dto, IJsonSerializer json,
        IApplicationHost host, ILogManager logs)
    {
        this.library = library;
        this.dto = dto;
        this.json = json;
        serverId = host.SystemId;
        logger = logs.GetLogger(Plugin.PluginName);
    }

    public string Name => "Sync library to Collect Metadata";
    public string Key => "CollectMetadataSyncLibrary";
    public string Category => Plugin.PluginName;
    public string Description => "Send movie, series, season and episode metadata to Collect Metadata.";

    private string? PreferredLanguage(BaseItem item)
    {
        var options = library.GetLibraryOptions(item);
        var language = item.GetPreferredMetadataLanguage(options)?.Trim().Replace('_', '-');
        if (string.IsNullOrEmpty(language) || !Regex.IsMatch(language, @"\A[a-zA-Z]{2,3}(?:-[a-zA-Z0-9]{2,4}){0,2}\z"))
        {
            return null; // Let Collect Metadata use its ordered language list.
        }
        var country = item.GetPreferredMetadataCountryCode(options)?.Trim();
        return language!.IndexOf('-') < 0 && country != null && Regex.IsMatch(country, @"\A[a-zA-Z]{2}\z")
            ? language + "-" + country
            : language;
    }

    public IEnumerable<TaskTriggerInfo> GetDefaultTriggers() => new[]
    {
        new TaskTriggerInfo { Type = TaskTriggerInfo.TriggerInterval, IntervalTicks = TimeSpan.FromMinutes(30).Ticks },
    };

    public async Task Execute(CancellationToken cancellationToken, IProgress<double> progress)
    {
        var config = Plugin.Instance?.Configuration;
        var service = Environment.GetEnvironmentVariable("COLLECT_METADATA_URL") ?? config?.CollectMetadataUrl;
        var token = Environment.GetEnvironmentVariable("EMBY_SYNC_TOKEN") ?? config?.SyncToken;
        if (string.IsNullOrWhiteSpace(service) && string.IsNullOrWhiteSpace(token))
        {
            return; // Optional integration; automatic scraping works on its own.
        }
        if (string.IsNullOrWhiteSpace(token) || !Uri.TryCreate(service, UriKind.Absolute, out var baseUri)
            || (baseUri.Scheme != Uri.UriSchemeHttp && baseUri.Scheme != Uri.UriSchemeHttps)
            || baseUri.UserInfo.Length != 0 || baseUri.Query.Length != 0 || baseUri.Fragment.Length != 0)
        {
            throw new InvalidOperationException("Configure the Collect Metadata HTTP(S) service URL and sync token.");
        }
        var endpoint = new Uri(baseUri.AbsoluteUri.TrimEnd('/') + "/api/emby/sync/" + Uri.EscapeDataString(serverId));
        var options = new DtoOptions(new[]
        {
            ItemFields.Overview, ItemFields.OriginalTitle, ItemFields.ExternalUrls, ItemFields.Genres,
            ItemFields.ParentId, ItemFields.People, ItemFields.ProductionLocations, ItemFields.ProviderIds,
            ItemFields.SortName, ItemFields.Studios, ItemFields.Taglines, ItemFields.Tags,
            ItemFields.RemoteTrailers, ItemFields.SeriesPrimaryImage, ItemFields.DateLastSaved,
        }) { EnableImages = true, EnableUserData = false };
        var ids = library.GetInternalItemIds(new InternalItemsQuery
        {
            IncludeItemTypes = new[] { "Movie", "Series", "Season", "Episode", "Video" },
            Recursive = true,
            IsVirtualItem = false,
        }, cancellationToken);
        var count = 0;
        var sent = 0;
        var rejected = 0;
        foreach (var id in ids)
        {
            cancellationToken.ThrowIfCancellationRequested();
            var item = library.GetItemById(id);
            if (item != null && !item.IsVirtualItem && !item.ExtraType.HasValue)
            {
                using var request = new HttpRequestMessage(HttpMethod.Post, endpoint);
                request.Headers.Authorization = new AuthenticationHeaderValue("Bearer", token!.Trim());
                var language = PreferredLanguage(item);
                if (language != null)
                {
                    request.Headers.Add("X-Emby-Metadata-Language", language);
                }
                request.Content = new StringContent(json.SerializeToString(dto.GetBaseItemDto(item, options)), Encoding.UTF8, "application/json");
                using var response = await Http.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, cancellationToken).ConfigureAwait(false);
                if (response.StatusCode == HttpStatusCode.NoContent)
                {
                    sent++;
                }
                else if (response.StatusCode == HttpStatusCode.BadRequest || response.StatusCode == HttpStatusCode.RequestEntityTooLarge)
                {
                    rejected++;
                    logger.Warn("Collect Metadata rejected Emby item {0} (HTTP {1}).", id, (int)response.StatusCode);
                }
                else
                {
                    throw new InvalidOperationException($"Collect Metadata sync failed (HTTP {(int)response.StatusCode}). The next run will retry.");
                }
            }
            progress.Report(100.0 * ++count / ids.Length);
        }
        logger.Info("Collect Metadata sync: {0} items acknowledged, {1} rejected.", sent, rejected);
        if (rejected != 0)
        {
            throw new InvalidOperationException("Some media items could not be synchronized. See the Emby log for item IDs.");
        }
    }
}
