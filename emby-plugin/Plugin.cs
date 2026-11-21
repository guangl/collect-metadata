using System;
using MediaBrowser.Common.Configuration;
using MediaBrowser.Common.Plugins;
using MediaBrowser.Model.Plugins;
using MediaBrowser.Model.Serialization;

namespace CollectMetadata.Emby;

public sealed class SyncConfiguration : BasePluginConfiguration
{
    public string CollectMetadataUrl { get; set; } = string.Empty;
    public string SyncToken { get; set; } = string.Empty;
}

public sealed class Plugin : BasePlugin<SyncConfiguration>
{
    internal static Plugin? Instance { get; private set; }

    public Plugin(IApplicationPaths paths, IXmlSerializer serializer) : base(paths, serializer)
    {
        Instance = this;
    }

    internal const string PluginName = "Collect Metadata - Auto Scrape";

    public override string Name => PluginName;
    public override string Description => "Automatically refresh metadata for new library items and synchronize metadata with Collect Metadata.";
    public override Guid Id { get; } = new Guid("0d06e6df-6aa6-4c32-94b3-ad9da31fa348");
}
