using System;
using System.Globalization;
using System.IO;
using MediaBrowser.Controller.Entities;
using MediaBrowser.Controller.Entities.Movies;
using MediaBrowser.Controller.Entities.TV;
using MediaBrowser.Controller.Library;
using MediaBrowser.Controller.Plugins;
using MediaBrowser.Model.Logging;

namespace CollectMetadata.Emby;

public sealed class AutoScrapeEntryPoint : IServerEntryPoint
{
    private readonly ILibraryManager libraryManager;
    private readonly ILogger logger;
    private readonly string? spool = Environment.GetEnvironmentVariable("EMBY_AUTO_SCRAPE_SPOOL");

    public AutoScrapeEntryPoint(
        ILibraryManager libraryManager,
        ILogManager logManager)
    {
        this.libraryManager = libraryManager;
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

        if (string.IsNullOrWhiteSpace(spool))
        {
            logger.Warn("Collect Metadata automatic scraping requires EMBY_AUTO_SCRAPE_SPOOL.");
            return;
        }

        var id = item.InternalId.ToString(CultureInfo.InvariantCulture);
        var path = Path.Combine(spool!, $"{item.GetType().Name}-{id}.event");
        try
        {
            // The file name is the entire event. Creating it is atomic and survives a restart.
            using var pending = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.Read);
            logger.Info("Queued Collect Metadata scrape for {0} {1}.", item.GetType().Name, id);
        }
        catch (IOException) when (File.Exists(path))
        {
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException)
        {
            logger.Error("Unable to queue Collect Metadata scrape for {0}: {1}", id, error.Message);
        }
    }
}
