using System;
using System.Globalization;
using System.IO;
using MediaBrowser.Controller.Entities;
using MediaBrowser.Controller.Entities.Movies;
using MediaBrowser.Controller.Entities.TV;
using MediaBrowser.Controller.Library;
using MediaBrowser.Controller.Plugins;
using MediaBrowser.Controller.Providers;
using MediaBrowser.Model.IO;
using MediaBrowser.Model.Logging;

namespace CollectMetadata.Emby;

public sealed class AutoScrapeEntryPoint : IServerEntryPoint
{
    private readonly ILibraryManager libraryManager;
    private readonly IProviderManager providerManager;
    private readonly IFileSystem fileSystem;
    private readonly ILogger logger;
    private readonly string? spool = Environment.GetEnvironmentVariable("EMBY_AUTO_SCRAPE_SPOOL");

    public AutoScrapeEntryPoint(
        ILibraryManager libraryManager,
        IProviderManager providerManager,
        IFileSystem fileSystem,
        ILogManager logManager)
    {
        this.libraryManager = libraryManager;
        this.providerManager = providerManager;
        this.fileSystem = fileSystem;
        logger = logManager.GetLogger(Plugin.PluginName);
    }

    public void Run()
    {
        libraryManager.ItemAdded += OnItemAdded;
        logger.Info("Listening for new movies, TV items, and home videos.");
    }

    public void Dispose() => libraryManager.ItemAdded -= OnItemAdded;

    private void OnItemAdded(object? sender, ItemChangeEventArgs args)
    {
        if (Plugin.Instance?.Configuration.AutoRefreshEnabled == false) return;
        var item = args.Item;
        if ((item is not (Movie or Series or Season or Episode) && item.GetType() != typeof(Video))
            || item.InternalId <= 0 || item.IsVirtualItem || item.IsLocked || item.ExtraType.HasValue)
        {
            return;
        }

        if (!string.IsNullOrWhiteSpace(spool))
        {
            var id = item.InternalId.ToString(CultureInfo.InvariantCulture);
            var path = Path.Combine(spool!, $"{item.GetType().Name}-{id}.event");
            try
            {
                // The file name is the entire event. Creating it is atomic and survives a restart.
                using var pending = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.Read);
                logger.Info("Queued Collect Metadata scrape for {0} {1}.", item.GetType().Name, id);
                return;
            }
            catch (IOException) when (File.Exists(path))
            {
                return;
            }
            catch (Exception error) when (error is IOException or UnauthorizedAccessException)
            {
                logger.Error("Unable to queue Collect Metadata scrape for {0}: {1}", id, error.Message);
            }
        }

        // Let Emby serialize and deduplicate pending refreshes; don't scrape on the event thread.
        var options = new MetadataRefreshOptions(fileSystem)
        {
            MetadataRefreshMode = MetadataRefreshMode.FullRefresh,
            ImageRefreshMode = MetadataRefreshMode.FullRefresh,
            ReplaceAllMetadata = false,
            ReplaceAllImages = false,
            IsAutomated = true,
            // New seasons and episodes have their own events. Don't rescan the whole series.
            Recursive = false,
        };
        providerManager.QueueRefresh(item.InternalId, options, RefreshPriority.Low);
        logger.Info("Requested metadata refresh for {0} {1}.", item.GetType().Name, item.InternalId);
    }
}
