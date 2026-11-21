using System;
using System.Collections.Generic;
using System.Linq;
using System.Xml.Serialization;
using MediaBrowser.Common.Configuration;
using MediaBrowser.Common.Plugins;
using MediaBrowser.Model.Plugins;
using MediaBrowser.Model.Serialization;

namespace CollectMetadata.Emby;

public sealed class SyncConfiguration : BasePluginConfiguration
{
    public string CollectMetadataUrl { get; set; } = string.Empty;
    public string SyncToken { get; set; } = string.Empty;
    public string OrganizeRoots { get; set; } = string.Empty;
    public bool OrganizeEnabled { get; set; } = true;
    public bool AutoRefreshEnabled { get; set; } = true;
    public bool SyncEnabled { get; set; } = true;
    public OrganizerRules Organizer { get; set; } = new OrganizerRules();

    [XmlIgnore] public string? EnvironmentRoots => Environment.GetEnvironmentVariable("EMBY_ORGANIZE_ROOTS");
    [XmlIgnore] public bool SyncUrlOverridden => Environment.GetEnvironmentVariable("COLLECT_METADATA_URL") != null;
    [XmlIgnore] public bool SyncTokenOverridden => Environment.GetEnvironmentVariable("EMBY_SYNC_TOKEN") != null;
}

public sealed class Plugin : BasePlugin<SyncConfiguration>, IHasWebPages
{
    internal static Plugin? Instance { get; private set; }

    public Plugin(IApplicationPaths paths, IXmlSerializer serializer) : base(paths, serializer)
    {
        Instance = this;
    }

    internal const string PluginName = "Collect Metadata - Auto Scrape";

    public override string Name => PluginName;
    public override string Description => "Refresh and synchronize metadata for library items; optionally organize loose movies and episodes.";
    public override Guid Id { get; } = new Guid("0d06e6df-6aa6-4c32-94b3-ad9da31fa348");

    public IEnumerable<PluginPageInfo> GetPages() => new[]
    {
        new PluginPageInfo { Name = "collectmetadata", DisplayName = "Collect Metadata · 自动整理",
            EmbeddedResourcePath = "CollectMetadata.Emby.Web.settings.html", MenuSection = "server", MenuIcon = "folder" },
        new PluginPageInfo { Name = "collectmetadata-config.js", IsMainConfigPage = false,
            EmbeddedResourcePath = "CollectMetadata.Emby.Web.settings.js" },
    };

    public override void UpdateConfiguration(BasePluginConfiguration configuration)
    {
        if (configuration is not SyncConfiguration settings) throw new ArgumentException("配置格式无效。");
        (settings.Organizer ??= new OrganizerRules()).Validate();
        var roots = OrganizeLibraryTask.SplitRoots(settings.OrganizeRoots ?? string.Empty).ToArray();
        if (roots.Length > 64 || Array.Exists(roots, root => OrganizeLibraryTask.NormalizeRoot(root) == null))
            throw new ArgumentException("请输入 Emby 中登记的绝对媒体库路径（每行一个，最多 64 个）。");
        settings.OrganizeRoots = string.Join(";", roots);
        var url = (settings.CollectMetadataUrl ?? string.Empty).Trim();
        if (url.Length != 0 && !SyncLibraryTask.ValidServiceUrl(url, out _))
            throw new ArgumentException("同步地址必须是有效的 HTTP(S) URL，不能包含凭据、查询或片段。");
        settings.CollectMetadataUrl = url;
        settings.SyncToken = (settings.SyncToken ?? string.Empty).Trim();
        base.UpdateConfiguration(settings);
    }
}
