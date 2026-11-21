define([], function () {
    "use strict";
    var pluginId = "0d06e6df-6aa6-4c32-94b3-ad9da31fa348";
    var defaults = { MovieFolder: "{original}", SeriesFolder: "{work}", SeasonFolder: "Season {season}", MovieFile: "{original}", EpisodeFile: "{original}" };
    var switchFields = ["OrganizeEnabled", "AutoRefreshEnabled", "SyncEnabled"];
    var moveFields = ["MoveNfo", "MoveSubtitles", "MoveImages", "RenameMovies", "RenameEpisodes"];
    var removeFields = ["RemoveYearFromFiles", "RemoveTmdbIdFromFiles"];
    var presets = {
        original: defaults,
        ani: { SeriesFolder: "{title}{year_suffix}{tmdb_suffix}", SeasonFolder: "Season ${season}", EpisodeFile: "${title} S${seasonFormat}E${episodeFormat}" },
        "ani-group": { SeriesFolder: "{title}{year_suffix}{tmdb_suffix}", SeasonFolder: "Season ${season}", EpisodeFile: "[${subgroup}] ${title} S${seasonFormat}E${episodeFormat}" },
        emby: { MovieFolder: "{title}{year_suffix}{tmdb_suffix}", MovieFile: "{title}{year_suffix}", SeriesFolder: "{title}{year_suffix}{tmdb_suffix}", SeasonFolder: "Season {season:00}", EpisodeFile: "{title} - {episode_code}" }
    };
    var year = /\s*\(((?:19|20)\d{2})\)/g;
    var tmdb = /\s*\[tmdbid[=-](\d+)\]/gi;
    var epPattern = /^(.+?)[ ._-]+(S(\d{1,3})[ ._-]*E(\d{1,4})(?:(?:[ ._-]*E\d{1,4}|-\d{1,4}(?![a-z0-9])))+|S(\d{1,3})[ ._-]*E(\d{1,4})|(\d{1,3})x(\d{1,4})(?:-(?:\d{1,3}x)?\d{1,4})?)(?=$|[^\p{L}\p{N}])/iu;
    var movie = sample("电影示例 (2025) [tmdbid=123].mkv", false);
    var episode = sample("剧集示例 (2024) S02E03.1080p.mkv", true);
    var identity = "original work title year year_suffix tmdbid tmdb_suffix";
    var scopes = { MovieFolder: identity, MovieFile: identity + " subgroup resolution", SeriesFolder: "work title year year_suffix tmdbid tmdb_suffix", SeasonFolder: "season season:00", EpisodeFile: identity + " season season:00 episode episode:00 episode_code subgroup resolution" };
    var required = { MovieFolder: /\{(?:original|work|title)\}/, SeriesFolder: /\{(?:work|title)\}/, SeasonFolder: /\{season(?::00)?\}/, EpisodeFile: /\{(?:original|episode_code|episode(?::00)?)\}/ };

    function splitRoots(value) { return value.split(/[;\r\n]+/).map(function (path) { return path.trim(); }).filter(Boolean); }
    function normalize(template) {
        return template.replaceAll("${seasonFormat}", "{season:00}").replaceAll("${episodeFormat}", "{episode:00}")
            .replaceAll("${itemTitle}", "{original}").replaceAll("${", "{");
    }
    function sample(file, series) {
        var original = file.trim().replace(/\.(mkv|mp4|avi|mov|m4v|wmv|ts|m2ts|webm|mpg|mpeg)$/i, "");
        if (!original || /[/\\]/.test(original)) return null;
        var match = series ? epPattern.exec(original) : null;
        if (series && !match) return null;
        var work = match ? match[1].replace(/[ ._-]+$/, "").replace(/[._]/g, " ") : original;
        var id = /\s*\[tmdbid[=-](\d+)\]\s*$/i.exec(work);
        var noId = id ? work.slice(0, id.index) : work;
        var date = /\s+\(((?:19|20)\d{2})\)\s*$/.exec(noId);
        var clean = date ? noId.slice(0, date.index) : noId;
        var subgroup = /^(?:\[([^\[\]]+)\]|【([^【】]+)】)\s*/.exec(clean);
        var out = { original: original, work: work, title: (subgroup ? clean.slice(subgroup[0].length) : clean).trim().replace(/_/g, " "),
            year: date ? date[1] : "", year_suffix: date ? " (" + date[1] + ")" : "", tmdbid: id ? id[1] : "", tmdb_suffix: id ? " [tmdbid=" + id[1] + "]" : "",
            subgroup: subgroup ? subgroup[1] || subgroup[2] : "", resolution: (original.match(/(?<![a-z0-9])(?:2160p|1440p|1080p|1080i|720p|576p|480p|4k)(?![a-z0-9])/i) || [""])[0],
            extension: file.trim().slice(original.length) || ".mkv" };
        if (match) {
            out.episode_code = match[2];
            out.season = String(Number(match[3] || match[5] || match[7]));
            out.episode = String(Number(match[4] || match[6] || match[8]));
            out["season:00"] = out.season.padStart(2, "0"); out["episode:00"] = out.episode.padStart(2, "0");
            out.multi = /[ex]\d+[- ._]*(?:e\d+|-\d+|-[0-9]+x)/i.test(out.episode_code);
            out.fractional = /^\.5(?=$|[^\p{L}\p{N}])/u.test(original.slice(match[0].length));
        }
        return out;
    }
    function render(template, values) {
        template = normalize(template);
        if (!values.subgroup) template = template.replaceAll("[{subgroup}]", "").replaceAll("【{subgroup}】", "");
        return template.replace(/\{([a-z0-9_:]+)\}/g, function (_, key) { return values[key] || ""; }).trim().replace(/[. ]+$/, "");
    }

    function templateError(key, template) {
        if (!template.trim() || /[/\\\x00-\x1f\x7f]/.test(template)) return "只允许填一个名称，不能包含路径分隔符。";
        template = normalize(template);
        if (/[{}]/.test(template.replace(/\{([a-z0-9_:]+)\}/g, function (token, name) {
            return scopes[key].split(" ").includes(name) ? "" : token;
        }))) return "模板中的标记无效，或不适用于此字段。";
        if (required[key] && !required[key].test(template)) return "此模板必须保留作品名或季集号等识别信息。";
        var shown = render(template, key.indexOf("Movie") === 0 ? movie : episode);
        if (!shown || /^[.@]/.test(shown)) return "生成的名称不能为空或以 . / @ 开头。";
        if (/File$/.test(key) && /\.(mkv|mp4|avi|mov|m4v|wmv|ts|m2ts|webm|mpg|mpeg)$/i.test(shown)) return "文件模板不填写扩展名，会自动沿用原扩展名。";
        if (key === "EpisodeFile" && !/(?:^|[ ._-])(?:s0?2[ ._-]*e0*3|0?2x0*3)(?:$|[^a-z0-9])/i.test(shown)) return "分集文件名必须生成可识别的季集号，例如 {title} - {episode_code}。";
        return "";
    }

    return function (view) {
        var api = this.getApiClient();
        var form = view.querySelector(".organizerSettings");
        var fields = form.elements;
        var feedback = view.querySelector(".organizer-feedback");
        var movieExample = view.querySelector(".movie-example");
        var seriesExample = view.querySelector(".series-example");
        var rootOverride = view.querySelector(".root-override");
        var syncOverride = view.querySelector(".sync-override");
        var config;

        function previewName(template, values, rename, series) {
            if (!rename || values.fractional) return values.original + values.extension;
            var result = normalize(template) === "{original}" ? values.original : render(template, values);
            if (fields.RemoveYearFromFiles.checked) result = result.replace(year, "").trim();
            if (fields.RemoveTmdbIdFromFiles.checked) result = result.replace(tmdb, "").trim();
            if (series) {
                var mapped = sample("Title " + result + values.extension, true);
                if (!mapped || mapped.season !== values.season || mapped.episode !== values.episode || ((values.multi || mapped.multi) && mapped.episode_code.toLowerCase() !== values.episode_code.toLowerCase())) result = values.original;
            }
            return result + values.extension;
        }
        function preview() {
            Object.keys(defaults).forEach(function (key) { fields[key].setCustomValidity(templateError(key, fields[key].value)); });
            var movieInput = sample(fields.SampleMovie.value, false), episodeInput = sample(fields.SampleEpisode.value, true);
            movieExample.textContent = movieInput ? render(fields.MovieFolder.value, movieInput) + "/" + previewName(fields.MovieFile.value, movieInput, fields.RenameMovies.checked, false) : "请输入视频文件名，不要填写路径。";
            seriesExample.textContent = episodeInput ? render(fields.SeriesFolder.value, episodeInput) + "/" + render(fields.SeasonFolder.value, episodeInput) + "/" + previewName(fields.EpisodeFile.value, episodeInput, fields.RenameEpisodes.checked, true) : "请填写包含 S02E03 或 2x03 等季集号的文件名。";
        }
        function fill(value) {
            config = value;
            var rules = config.Organizer || {};
            switchFields.forEach(function (key) { fields[key].checked = config[key] !== false; });
            moveFields.forEach(function (key) { fields[key].checked = rules[key] !== false; });
            removeFields.forEach(function (key) { fields[key].checked = rules[key] === true; });
            Object.keys(defaults).forEach(function (key) { fields[key].value = rules[key] == null ? defaults[key] : rules[key]; });
            fields.StableSeconds.value = rules.StableSeconds == null ? 60 : rules.StableSeconds;
            fields.OrganizeRoots.value = splitRoots(config.EnvironmentRoots == null ? config.OrganizeRoots || "" : config.EnvironmentRoots).join("\n");
            fields.CollectMetadataUrl.value = config.CollectMetadataUrl || "";
            fields.SyncToken.value = "";
            fields.SyncToken.placeholder = config.SyncToken ? "已保存，留空沿用" : "未配置令牌";
            fields.ClearToken.checked = false;
            Array.from(form.querySelectorAll("fieldset")).forEach(function (field) { field.disabled = false; });
            fields.Save.disabled = false;
            var envRoots = config.EnvironmentRoots != null;
            fields.OrganizeRoots.readOnly = envRoots;
            fields.RootPicker.disabled = fields.AddRoot.disabled = envRoots;
            rootOverride.hidden = !envRoots;
            rootOverride.textContent = "当前路径由容器的 EMBY_ORGANIZE_ROOTS 环境变量指定。移除此变量后，才会使用页面保存的路径。开关和模板仍可在此修改。";
            fields.CollectMetadataUrl.readOnly = !!config.SyncUrlOverridden;
            fields.SyncToken.disabled = fields.ClearToken.disabled = !!config.SyncTokenOverridden;
            syncOverride.hidden = !config.SyncUrlOverridden && !config.SyncTokenOverridden;
            syncOverride.textContent = "标为只读或禁用的同步字段由环境变量提供。";
            preview();
        }
        feedback.textContent = "正在读取插件配置…";
        api.getPluginConfiguration(pluginId).then(function (value) {
            fill(value);
            feedback.textContent = "";
        }).catch(function () { feedback.textContent = "无法读取配置。请确认已用 Emby 管理员账号登录。"; });

        api.getVirtualFolders().then(function (result) {
            var seen = new Set();
            result.Items.forEach(function (library) {
                if (!/^(movies|tvshows)$/i.test(library.CollectionType || "")) return;
                (library.Locations || []).forEach(function (path) {
                    if (seen.has(path)) return;
                    seen.add(path);
                    var option = document.createElement("option");
                    option.value = path;
                    option.textContent = library.Name + " · " + path;
                    fields.RootPicker.appendChild(option);
                });
            });
        }).catch(function () { /* Paths may still be entered directly. */ });
        fields.AddRoot.addEventListener("click", function () {
            var path = fields.RootPicker.value;
            if (path && !fields.OrganizeRoots.readOnly) fields.OrganizeRoots.value = Array.from(new Set(splitRoots(fields.OrganizeRoots.value).concat(path))).join("\n");
        });
        function tokenButtons() {
            var target = fields.TokenTarget.value;
            var host = view.querySelector(".cm-token-buttons");
            host.replaceChildren();
            scopes[target].split(" ").forEach(function (key) {
                var token = key === "season:00" ? "${seasonFormat}" : key === "episode:00" ? "${episodeFormat}" : "{" + key + "}";
                var button = document.createElement("button");
                button.type = "button";
                button.className = "cm-button";
                button.textContent = token;
                button.addEventListener("click", function () {
                    var field = fields[target];
                    field.setRangeText(token, field.selectionStart, field.selectionEnd, "end");
                    field.focus(); preview();
                });
                host.appendChild(button);
            });
        }
        tokenButtons();
        fields.TokenTarget.addEventListener("change", tokenButtons);
        fields.ApplyPreset.addEventListener("click", function () {
            var preset = presets[fields.NamingPreset.value];
            Object.keys(preset).forEach(function (key) { fields[key].value = preset[key]; });
            if (preset.MovieFile) fields.RenameMovies.checked = true;
            if (preset.EpisodeFile) fields.RenameEpisodes.checked = true;
            fields.RemoveYearFromFiles.checked = fields.RemoveTmdbIdFromFiles.checked = false;
            preview();
        });
        fields.Defaults.addEventListener("click", function () {
            Object.keys(defaults).forEach(function (key) { fields[key].value = defaults[key]; });
            preview();
        });
        form.addEventListener("input", preview);
        form.addEventListener("submit", function (event) {
            event.preventDefault();
            if (!config || fields.Save.disabled) return;
            preview();
            if (!form.reportValidity()) return;
            var next = Object.assign({}, config);
            delete next.EnvironmentRoots;
            delete next.SyncUrlOverridden;
            delete next.SyncTokenOverridden;
            switchFields.forEach(function (key) { next[key] = fields[key].checked; });
            if (!fields.OrganizeRoots.readOnly) next.OrganizeRoots = splitRoots(fields.OrganizeRoots.value).join(";");
            if (!fields.CollectMetadataUrl.readOnly) next.CollectMetadataUrl = fields.CollectMetadataUrl.value.trim();
            if (!fields.SyncToken.disabled) next.SyncToken = fields.ClearToken.checked ? "" : fields.SyncToken.value.trim() || config.SyncToken || "";
            next.Organizer = { StableSeconds: Number(fields.StableSeconds.value) };
            Object.keys(defaults).forEach(function (key) { next.Organizer[key] = fields[key].value.trim(); });
            moveFields.concat(removeFields).forEach(function (key) { next.Organizer[key] = fields[key].checked; });
            fields.Save.disabled = true;
            feedback.textContent = "正在保存…";
            api.updatePluginConfiguration(pluginId, next).then(function () { return api.getPluginConfiguration(pluginId); }).then(function (value) {
                fill(value);
                feedback.textContent = "已保存。下一次计划任务会读取新设置；不会重命名已在子目录里的作品。";
            }).catch(function () {
                feedback.textContent = "未能确认保存。请检查媒体库路径、命名模板和同步地址，或查看 Emby 日志。";
                fields.Save.disabled = false;
            });
        });
    };
});
