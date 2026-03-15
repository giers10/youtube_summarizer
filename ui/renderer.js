const tauriApi = window.__TAURI__;
const invoke = tauriApi?.core?.invoke;
const listen = tauriApi?.event?.listen;
const convertFileSrc = tauriApi?.core?.convertFileSrc;
const confirmDialog = tauriApi?.dialog?.confirm;

if (!invoke || !listen) {
  throw new Error('Tauri runtime API is unavailable.');
}

function toWebviewFileUrl(filePath) {
  if (!filePath) {
    return filePath;
  }
  if (typeof convertFileSrc === 'function') {
    return convertFileSrc(filePath);
  }
  return filePath;
}

window.api = {
  getModels: () => invoke('get_models'),
  getSummaries: () => invoke('get_summaries'),
  summarizeVideo: (url, useWhisper, model) => invoke('summarize_video', {
    request: {
      url,
      useWhisper,
      model: model || null
    }
  }),
  openExternal: (url) => invoke('open_external', { url }),
  openFile: (filePath) => invoke('open_file', { filePath }),
  deleteSummary: (id) => invoke('delete_summary', {
    request: { id }
  }),
  translateSummary: (id, lang, model) => invoke('translate_summary', {
    request: {
      id,
      lang,
      model: model || null
    }
  }),
  onSummarizeProgress: (callback) => listen('summarize-progress', (event) => {
    callback(String(event.payload || ''));
  })
};

window.addEventListener('DOMContentLoaded', async () => {
  const form = document.getElementById('summarize-form');
  const urlInput = document.getElementById('url-input');
  const whisperCheckbox = document.getElementById('whisper-checkbox');
  const summariesContainer = document.getElementById('summaries-container');
  const loadingIndicator = document.getElementById('loading');
  const modelSelect = document.getElementById('model-select');
  const paginationTop = document.getElementById('pagination-top');
  const paginationBottom = document.getElementById('pagination-bottom');
  const summarizeButton = form.querySelector('button[type="submit"]');
  const autoTranslateCheckbox = document.getElementById('autotranslate-checkbox');

  let fullSummaries = [];
  let currentPage = 1;
  const PAGE_SIZE = 20;
  let isLoading = false;
  let entryUiState = {};

  function setLoadingMessage(message) {
    if (!isLoading) {
      return;
    }
    loadingIndicator.style.display = 'inline';
    loadingIndicator.textContent = message;
  }

  whisperCheckbox.checked = localStorage.getItem('useWhisper') === '0' ? false : true;
  autoTranslateCheckbox.checked = localStorage.getItem('autoTranslate') === '1' ? true : false;

  whisperCheckbox.addEventListener('change', () => {
    localStorage.setItem('useWhisper', whisperCheckbox.checked ? '1' : '0');
  });
  autoTranslateCheckbox.addEventListener('change', () => {
    localStorage.setItem('autoTranslate', autoTranslateCheckbox.checked ? '1' : '0');
  });

  function renderSummaries(list) {
    summariesContainer.innerHTML = '';
    const renderedIds = new Set();

    list.forEach(item => {
      renderedIds.add(item.id);
      if (!entryUiState[item.id]) {
        entryUiState[item.id] = { expanded: false, lang: 'en' };
      }
      let { expanded, lang } = entryUiState[item.id];

      const entry = document.createElement('div');
      entry.classList.add('entry');
      entry.style.overflow = 'hidden';

      const deleteButton = document.createElement('button');
      deleteButton.type = 'button';
      deleteButton.innerHTML = '&times;';
      deleteButton.classList.add('delete-entry-button');
      deleteButton.style.width = '24px';
      deleteButton.style.height = '24px';
      deleteButton.style.display = 'flex';
      deleteButton.style.alignItems = 'center';
      deleteButton.style.justifyContent = 'center';
      deleteButton.style.border = 'none';
      deleteButton.style.background = 'transparent';
      deleteButton.style.color = '#9f1239';
      deleteButton.style.fontSize = '22px';
      deleteButton.style.fontWeight = 'normal';
      deleteButton.style.cursor = 'pointer';
      deleteButton.style.padding = '0';
      deleteButton.style.lineHeight = '1';
      deleteButton.disabled = isLoading;
      deleteButton.addEventListener('click', (e) => {
        e.preventDefault();
        e.stopPropagation();
        if (isLoading) {
          return;
        }
        if (typeof confirmDialog !== 'function') {
          alert('Delete confirmation is unavailable.');
          return;
        }
        confirmDialog('Are you sure you want to delete this entry?', {
          title: 'Delete entry',
          kind: 'warning'
        }).then((confirmed) => {
          if (!confirmed) {
            return;
          }
          window.api.deleteSummary(item.id)
            .then(() => {
              delete entryUiState[item.id];
              return window.api.getSummaries().then(setSummaries);
            })
            .catch(err => {
              alert('Error deleting summary: ' + err.message);
            });
        });
      });
      const left = document.createElement('div');
      left.classList.add('left');
      if (item.thumbnail) {
        const img = document.createElement('img');
        img.src = toWebviewFileUrl(item.thumbnail);
        img.alt = item.video_name;
        img.classList.add('thumbnail');
        if (item.url) {
          img.style.cursor = 'pointer';
          img.title = 'Open video';
          img.addEventListener('click', (e) => {
            e.stopPropagation();
            window.api.openExternal(item.url);
          });
        }
        left.appendChild(img);
      }

      const langSwitcher = document.createElement('span');
      langSwitcher.style.display = 'flex';
      langSwitcher.style.gap = '6px';
      langSwitcher.style.marginTop = '8px';
      langSwitcher.style.marginBottom = '2px';

      const summaryFields = {
        en: item.summary_en,
        de: item.summary_de,
        jp: item.summary_jp
      };

      ['en', 'de', 'jp'].forEach(thisLang => {
        const btn = document.createElement('button');
        btn.type = 'button';
        btn.textContent = thisLang.toUpperCase();
        btn.style.fontSize = '12px';
        btn.style.padding = '2px 8px';
        btn.style.borderRadius = '5px';
        btn.style.border = '1px solid #eee';
        btn.style.background = (thisLang === lang) ? '#9f1239' : '#fff1f2';
        btn.style.color = (thisLang === lang) ? '#fff' : '#9f1239';
        btn.disabled = isLoading;
        btn.addEventListener('click', () => {
          lang = thisLang;
          entryUiState[item.id].lang = lang;
          renderSummaryContent();
          Array.from(langSwitcher.children).forEach((button, index) => {
            const language = ['en', 'de', 'jp'][index];
            button.style.background = (language === lang) ? '#9f1239' : '#fff1f2';
            button.style.color = (language === lang) ? '#fff' : '#9f1239';
          });
        });
        langSwitcher.appendChild(btn);
      });
      left.appendChild(langSwitcher);

      const middle = document.createElement('div');
      middle.classList.add('middle');
      const headline = document.createElement('div');
      headline.style.display = 'flex';
      headline.style.alignItems = 'center';
      headline.style.justifyContent = 'space-between';
      headline.style.gap = '12px';
      const headlineMain = document.createElement('div');
      headlineMain.style.display = 'flex';
      headlineMain.style.alignItems = 'center';
      headlineMain.style.minWidth = '0';
      const titleEl = document.createElement('strong');
      titleEl.style.display = 'block';
      titleEl.style.fontSize = '16px';
      titleEl.style.cursor = 'default';
      titleEl.style.marginLeft = '0';
      titleEl.textContent = item.video_name;

      const arrow = document.createElement('span');
      arrow.textContent = expanded ? '▼' : '▶';
      arrow.style.marginRight = '8px';
      arrow.style.marginLeft = '0';
      arrow.style.fontSize = '18px';
      arrow.style.userSelect = 'none';
      arrow.style.transition = 'transform 0.15s';

      headlineMain.appendChild(arrow);
      headlineMain.appendChild(titleEl);
      headline.appendChild(headlineMain);
      headline.appendChild(deleteButton);

      const channelEl = document.createElement('span');
      channelEl.style.fontSize = '14px';
      channelEl.style.opacity = '0.8';
      channelEl.style.marginBottom = '12px';
      channelEl.textContent = item.channel || '';
      channelEl.style.display = 'block';
      channelEl.style.marginTop = '2px';

      middle.appendChild(headline);
      middle.appendChild(channelEl);

      const summaryHTML = document.createElement('div');
      summaryHTML.classList.add('summary');
      summaryHTML.style.display = '-webkit-box';
      summaryHTML.style.webkitBoxOrient = 'vertical';
      summaryHTML.style.overflow = 'hidden';
      summaryHTML.style.transition = 'max-height 0.2s';

      function renderSummaryContent() {
        const text = summaryFields[lang];
        summaryHTML.innerHTML = '';
        if (text && text.trim()) {
          summaryHTML.innerHTML = markdownToHTML(text);
        } else {
          const missingMsg = document.createElement('span');
          missingMsg.textContent = (
            lang === 'de' ? 'German not available. ' :
            lang === 'jp' ? 'Japanese not available. ' :
            'Not available. '
          );
          summaryHTML.appendChild(missingMsg);
        }
        if (!expanded) {
          summaryHTML.style.webkitLineClamp = '2';
          summaryHTML.style.maxHeight = '2.8em';
        } else {
          summaryHTML.style.webkitLineClamp = '';
          summaryHTML.style.maxHeight = '';
        }
      }

      middle.appendChild(summaryHTML);

      entry.appendChild(left);
      entry.appendChild(middle);

      summariesContainer.appendChild(entry);

      function applyCollapsedStyle() {
        if (!expanded) {
          entry.classList.add('collapsed');
          arrow.textContent = '▶';
        } else {
          entry.classList.remove('collapsed');
          arrow.textContent = '▼';
        }
        renderSummaryContent();
      }
      applyCollapsedStyle();

      middle.addEventListener('click', () => {
        if (!expanded) {
          expanded = true;
          entryUiState[item.id].expanded = true;
          applyCollapsedStyle();
        }
      });

      headline.addEventListener('click', (e) => {
        if (expanded) {
          expanded = false;
          entryUiState[item.id].expanded = false;
          applyCollapsedStyle();
          e.stopPropagation();
        }
      });
    });

    Object.keys(entryUiState).forEach(id => {
      if (!renderedIds.has(Number(id))) {
        delete entryUiState[id];
      }
    });

    setActionLinksDisabled(isLoading);
  }

  function markdownToHTML(text) {
    text = text.replace(/<\/think(?:ing)?>[^\S\n]*\n+[^\S\n]*/gi, '</think>');
    text = text.replace(
      /(^|\n)\s*<think>[\s\S]*?<\/think(?:ing)?>\s*(\n\s*\n)?/gi,
      (_, lead) => (lead ? '\n' : '')
    );

    let tmp = text.replace(/\r\n/g, '\n').replace(/\r/g, '\n');
    tmp = tmp.replace(
      /(^|\n)\s*<think>[\s\S]*?<\/think(?:ing)?>\s*(?=\n|$)/gi,
      (_, lead) => (lead ? '\n' : '')
    );

    const codeblocks = [];
    const placeholder = idx => `@@CODEBLOCK${idx}@@`;
    tmp = tmp.replace(/```([\s\S]*?)```/g, (_, code) => {
      codeblocks.push(code);
      return placeholder(codeblocks.length - 1);
    });

    let escaped = tmp
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;');

    escaped = escaped
      .replace(/^#### (.+)$/gm, '<h4>$1</h4>')
      .replace(/^### (.+)$/gm, '<h3>$1</h3>')
      .replace(/^## (.+)$/gm, '<h2>$1</h2>')
      .replace(/^# (.+)$/gm, '<h1>$1</h1>');

    escaped = escaped.replace(
      /(^|\n)([ \t]*\* .+(?:\n[ \t]*\* .+)*)/g,
      (_, lead, listBlock) => {
        const items = listBlock
          .split(/\n/)
          .map(line => line.replace(/^[ \t]*\*\s+/, '').trim())
          .map(item => `<li>${item}</li>`)
          .join('');
        return `${lead}<ul>${items}</ul>`;
      }
    );

    let html = escaped
      .replace(/\*\*(.+?)\*\*/g, '<b>$1</b>')
      .replace(/(?<!\*)\*(.+?)\*(?!\*)/g, '<i>$1</i>')
      .replace(/`(.+?)`/g, '<code>$1</code>');

    html = html.replace(/@@CODEBLOCK(\d+)@@/g, (_, idx) => {
      const code = codeblocks[Number(idx)];
      return `<pre><code>${code}</code></pre>`;
    });

    html = html.replace(/\n*(<h[1-3]>.*?<\/h[1-3]>)\n*/g, '$1\n');
    html = html.replace(/\n/g, '<br>');
    html = html
      .replace(/<br>\s*(<h[1-3]>)/g, '$1')
      .replace(/(<\/h[1-3]>)\s*<br>/g, '$1');

    return html;
  }

  function setActionLinksDisabled(disabled) {
    document.querySelectorAll('.delete-entry-button').forEach(button => {
      if (disabled) {
        button.disabled = true;
        button.style.opacity = '0.5';
      } else {
        button.disabled = false;
        button.style.opacity = '';
      }
    });
    document.querySelectorAll('.left button').forEach(btn => {
      btn.disabled = disabled;
      btn.style.opacity = disabled ? '0.5' : '';
    });
  }

  function updatePaginationControls() {
    if (!fullSummaries || fullSummaries.length <= PAGE_SIZE) {
      paginationTop.style.display = 'none';
      paginationBottom.style.display = 'none';
      return;
    }
    paginationTop.style.display = 'flex';
    paginationBottom.style.display = 'flex';
    const totalPages = Math.ceil(fullSummaries.length / PAGE_SIZE);

    const buildNav = (container) => {
      container.innerHTML = '';

      const prevBtn = document.createElement('button');
      prevBtn.textContent = '«';
      prevBtn.disabled = currentPage === 1;
      prevBtn.addEventListener('click', () => {
        if (currentPage > 1) {
          showPage(currentPage - 1);
          updatePaginationControls();
        }
      });
      container.appendChild(prevBtn);

      for (let i = 1; i <= totalPages; i += 1) {
        const btn = document.createElement('button');
        btn.textContent = i;
        if (i === currentPage) {
          btn.classList.add('active');
        }
        btn.addEventListener('click', () => {
          showPage(i);
          updatePaginationControls();
        });
        container.appendChild(btn);
      }

      const nextBtn = document.createElement('button');
      nextBtn.textContent = '»';
      nextBtn.disabled = currentPage === totalPages;
      nextBtn.addEventListener('click', () => {
        if (currentPage < totalPages) {
          showPage(currentPage + 1);
          updatePaginationControls();
        }
      });
      container.appendChild(nextBtn);
    };

    buildNav(paginationTop);
    buildNav(paginationBottom);
  }

  function showPage(page) {
    const totalPages = Math.ceil(fullSummaries.length / PAGE_SIZE);
    currentPage = Math.max(1, Math.min(page, totalPages || 1));
    const start = (currentPage - 1) * PAGE_SIZE;
    const end = start + PAGE_SIZE;
    renderSummaries(fullSummaries.slice(start, end));
  }

  function setSummaries(list) {
    fullSummaries = list || [];
    const totalPages = Math.ceil(fullSummaries.length / PAGE_SIZE);
    if (currentPage > totalPages) {
      currentPage = Math.max(1, totalPages);
    }
    showPage(currentPage);
    updatePaginationControls();
  }

  try {
    const models = await window.api.getModels();
    modelSelect.innerHTML = '';
    const hasMistral = Array.isArray(models) && models.includes('mistral:latest');
    const placeholder = document.createElement('option');
    placeholder.disabled = true;
    placeholder.value = '';
    placeholder.innerText = 'Select model';
    modelSelect.appendChild(placeholder);
    if (Array.isArray(models)) {
      models.forEach(name => {
        const option = document.createElement('option');
        option.value = name;
        option.innerText = name;
        modelSelect.appendChild(option);
      });
    }
    const saved = localStorage.getItem('selectedModel');
    let toSelect = '';
    if (saved && models.includes(saved)) {
      toSelect = saved;
    } else if (hasMistral) {
      toSelect = 'mistral:latest';
    }
    if (toSelect) {
      modelSelect.value = toSelect;
      placeholder.selected = false;
    } else {
      placeholder.selected = true;
    }
  } catch (err) {
    console.error('Error loading models:', err);
    modelSelect.innerHTML = '';
    const placeholder = document.createElement('option');
    placeholder.disabled = true;
    placeholder.selected = true;
    placeholder.value = '';
    placeholder.innerText = 'Select model';
    modelSelect.appendChild(placeholder);
  }

  modelSelect.addEventListener('change', () => {
    localStorage.setItem('selectedModel', modelSelect.value);
  });

  window.api.getSummaries().then(setSummaries).catch(console.error);

  form.addEventListener('submit', (e) => {
    e.preventDefault();
    const url = urlInput.value.trim();
    const useWhisper = whisperCheckbox.checked;
    const autoTranslate = autoTranslateCheckbox.checked;
    if (!url || isLoading) {
      return;
    }

    isLoading = true;
    summarizeButton.disabled = true;
    setLoadingMessage('Summarizing…');
    setActionLinksDisabled(true);

    const selectedModel = modelSelect.value;
    window.api.summarizeVideo(url, useWhisper, selectedModel)
      .then((newEntry) => {
        if (!newEntry || !newEntry.id) {
          return window.api.getSummaries().then(setSummaries);
        }

        entryUiState[newEntry.id] = { expanded: true, lang: 'en' };

        if (!autoTranslate) {
          return window.api.getSummaries().then(setSummaries);
        }

        let translationsOk = true;
        setLoadingMessage('Translating to German (DE)…');
        return window.api.translateSummary(newEntry.id, 'de', selectedModel)
          .then(() => {
            setLoadingMessage('Translating to Japanese (JP)…');
            return window.api.translateSummary(newEntry.id, 'jp', selectedModel);
          })
          .catch(err => {
            translationsOk = false;
            alert('Error translating summary: ' + err.message);
          })
          .then(() => {
            entryUiState[newEntry.id] = {
              expanded: true,
              lang: translationsOk ? 'jp' : 'en'
            };
            return window.api.getSummaries().then(setSummaries);
          });
      })
      .catch(err => {
        alert('Error summarizing video: ' + err.message);
      })
      .finally(() => {
        loadingIndicator.style.display = 'none';
        loadingIndicator.textContent = 'Loading…';
        summarizeButton.disabled = false;
        isLoading = false;
        setActionLinksDisabled(false);
        urlInput.value = '';
      });
  });

  window.api.onSummarizeProgress(line => {
    if (!isLoading || !line) {
      return;
    }
    setLoadingMessage(line);
  });
});
