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
