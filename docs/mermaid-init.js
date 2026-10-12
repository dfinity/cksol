(function () {
  const darkThemes = ["navy", "coal", "ayu"];

  function isDarkTheme() {
    return darkThemes.some((theme) => document.documentElement.classList.contains(theme));
  }

  function replaceCodeBlocksWithMermaidContainers() {
    for (const code of document.querySelectorAll("code.language-mermaid")) {
      const container = document.createElement("pre");
      container.className = "mermaid";
      container.textContent = code.textContent;
      code.parentElement.replaceWith(container);
    }
  }

  import("https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs").then(({ default: mermaid }) => {
    replaceCodeBlocksWithMermaidContainers();
    mermaid.initialize({ startOnLoad: false, theme: isDarkTheme() ? "dark" : "default" });
    mermaid.run();
  });
})();
