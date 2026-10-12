(function () {
  const mermaidModuleUrl = "https://cdn.jsdelivr.net/npm/mermaid@11.17.2/dist/mermaid.esm.min.mjs";
  const darkThemes = ["navy", "coal", "ayu"];

  function isDarkTheme() {
    return darkThemes.some((theme) => document.documentElement.classList.contains(theme));
  }

  function replaceCodeBlocksWithMermaidContainers() {
    const containers = [];
    for (const code of document.querySelectorAll("code.language-mermaid")) {
      const container = document.createElement("pre");
      container.className = "mermaid";
      container.dataset.source = code.textContent;
      code.parentElement.replaceWith(container);
      containers.push(container);
    }
    return containers;
  }

  function render(mermaid, containers) {
    for (const container of containers) {
      container.textContent = container.dataset.source;
      container.removeAttribute("data-processed");
    }
    mermaid.initialize({ startOnLoad: false, theme: isDarkTheme() ? "dark" : "default" });
    mermaid.run({ nodes: containers });
  }

  function rerenderOnThemeChange(mermaid, containers) {
    let wasDark = isDarkTheme();
    new MutationObserver(() => {
      const isDark = isDarkTheme();
      if (isDark !== wasDark) {
        wasDark = isDark;
        render(mermaid, containers);
      }
    }).observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
  }

  import(mermaidModuleUrl).then(({ default: mermaid }) => {
    const containers = replaceCodeBlocksWithMermaidContainers();
    if (containers.length === 0) {
      return;
    }
    render(mermaid, containers);
    rerenderOnThemeChange(mermaid, containers);
  });
})();
