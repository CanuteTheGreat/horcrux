# Horcrux Website

This directory contains the static website for Horcrux, deployed via GitHub Pages.

## Structure

```
website/
├── index.html          # Main landing page
├── docs.html           # Documentation portal
├── css/
│   └── style.css       # All styles
├── js/
│   └── main.js         # Interactive features
└── images/             # Image assets (if needed)
```

## Features

### Landing Page (index.html)
- Hero section with terminal animation
- Key differentiators showcase
- Comprehensive feature listing
- Quick start with Docker/Source/Gentoo tabs
- Horcrux vs Proxmox comparison table
- Performance statistics
- Call-to-action section

### Documentation Portal (docs.html)
- Organized documentation links
- Getting Started guides
- Deployment documentation
- Configuration examples
- API reference
- Operations guides
- Security documentation
- Advanced topics
- Development resources

### Styling (CSS)
- Modern dark theme with purple/cyan accents
- Fully responsive design
- Smooth animations and transitions
- Terminal styling for code blocks
- Interactive hover effects
- Mobile-friendly navigation

### JavaScript Features
- Tab switching for quick start section
- Smooth scrolling for anchor links
- Scroll animations for cards
- Code copy-to-clipboard buttons
- Parallax effects
- Terminal typing animation
- Mobile menu toggle
- Easter egg (Konami code!)

## Local Development

To test the website locally:

```bash
# Using Python's built-in server
cd docs/website
python3 -m http.server 8000

# Then open http://localhost:8000 in your browser
```

Or use any static file server:

```bash
# Using npx (Node.js)
npx serve docs/website

# Using PHP
php -S localhost:8000 -t docs/website
```

## Deployment

### GitHub Pages (Automatic)

The website is automatically deployed to GitHub Pages when changes are pushed to the `main` branch.

**Setup Instructions:**

1. Go to your repository settings
2. Navigate to **Pages** section
3. Under **Source**, select "GitHub Actions"
4. The workflow in `.github/workflows/pages.yml` will handle deployment

Your site will be available at: `https://canutethegreat.github.io/horcrux/`

### Manual Deployment

You can also deploy to any static hosting service:

```bash
# Copy website files to hosting
rsync -avz docs/website/ user@yourserver:/var/www/horcrux/

# Or upload via FTP/SFTP
# Or use services like Netlify, Vercel, Cloudflare Pages
```

## Customization

### Changing Colors

Edit the CSS variables in `css/style.css`:

```css
:root {
    --primary-color: #a855f7;      /* Purple */
    --secondary-color: #f97316;    /* Orange */
    --accent-color: #06b6d4;       /* Cyan */
    --bg-color: #0f172a;           /* Dark blue */
    /* ... */
}
```

### Adding Pages

1. Create new HTML file in `docs/website/`
2. Copy header/footer from existing pages
3. Add navigation link in navbar
4. Follow existing styling patterns

### Adding Images

1. Add images to `docs/website/images/`
2. Reference in HTML: `<img src="images/yourimage.png" alt="Description">`
3. Optimize images for web (use WebP format when possible)

## Browser Support

The website is tested and works on:
- Chrome/Edge (latest)
- Firefox (latest)
- Safari (latest)
- Mobile browsers (iOS Safari, Chrome Mobile)

## Performance

- Optimized CSS with minimal dependencies
- No external JavaScript libraries
- Lightweight (~50KB total without images)
- Fast load times
- Excellent Lighthouse scores

## Accessibility

- Semantic HTML structure
- ARIA labels where needed
- Keyboard navigation support
- High contrast ratios
- Responsive text sizing

## SEO

- Meta descriptions on all pages
- Semantic heading structure
- Open Graph tags (can be added)
- Sitemap (can be generated)

## Future Enhancements

Potential additions:
- [ ] Screenshot carousel/gallery
- [ ] Video demos
- [ ] Interactive API explorer
- [ ] Live demo environment
- [ ] Blog section
- [ ] Search functionality
- [ ] Dark/light theme toggle
- [ ] Internationalization (i18n)

## Contributing

To contribute to the website:

1. Make changes to files in `docs/website/`
2. Test locally using a static server
3. Ensure responsive design works (test on mobile)
4. Commit changes and create PR
5. Preview will be available via GitHub Pages workflow

## License

The website content is part of the Horcrux project and is licensed under GPL v3.

---

**Live Site:** https://canutethegreat.github.io/horcrux/ (once deployed)

**Need Help?** [Open an issue](https://github.com/CanuteTheGreat/horcrux/issues)
