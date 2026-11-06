# Лендинг «Подтекст»

Статическая страница без сборки и зависимостей. Лежит в `site/`, деплой идёт
через GitHub Actions.

## Публикация

1. Откройте Settings -> Pages.
2. В Source выберите GitHub Actions.
3. Запустите workflow `pages` или сделайте push в `main`.

Страница будет доступна по адресу `https://guest363.github.io/Podtekst/`.
Проверьте фактический регистр пути после первого деплоя.

## Что обновить при смене адреса

- `site/index.html`: `canonical`, `og:url`, `og:image`, `twitter:image`, JSON-LD.
- `site/robots.txt` и `site/sitemap.xml`.

## Файлы

- `index.html` - разметка и тексты.
- `styles.css` - оформление, адаптив.
- `app.js` - меню и определение ОС для кнопки.
- `assets/` - иконка и соцпревью.

Сайт не использует cookies, аналитику и внешние шрифты.
