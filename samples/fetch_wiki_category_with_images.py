#!/usr/bin/env python3
"""
fetch_wiki_category_with_images.py

Fetches all pages from a Wikipedia category AND retrieves the image file names
included on each page.
"""

import time
import requests
from typing import List, Dict, Optional

# ---------- Configuration ----------
WIKIPEDIA_API_URL = "https://en.wikipedia.org/w/api.php"
CATEGORY_TITLE = "Category:National identity cards by country"
CM_LIMIT = 50          # Pages per request (max 500)
IMAGE_LIMIT = 20       # Images per page (max 500)
REQUEST_DELAY = 1      # Delay in seconds to respect API etiquette
HEADERS = {
    "User-Agent": "MyWikipediaFetcher/1.0 (https://mywebsite.com/contact; myemail@example.com)"
}
# -----------------------------------


def fetch_category_members(session: requests.Session, cm_title: str, cm_limit: int = 50) -> List[str]:
    """
    Fetch all page titles under a given category (handles pagination).
    """
    all_titles: List[str] = []
    continue_param: Optional[str] = None

    while True:
        params = {
            "action": "query",
            "format": "json",
            "list": "categorymembers",
            "cmtitle": cm_title,
            "cmlimit": cm_limit,
            "cmtype": "page",       # Only regular pages, no subcategories
        }
        if continue_param:
            params["cmcontinue"] = continue_param

        try:
            response = session.get(WIKIPEDIA_API_URL, params=params, headers=HEADERS)
            response.raise_for_status()
            data = response.json()
        except Exception as e:
            print(f"Error fetching category members: {e}")
            break

        pages = data.get("query", {}).get("categorymembers", [])
        for page in pages:
            if page.get("ns") == 0:  # Main namespace only
                all_titles.append(page["title"])

        continue_param = data.get("continue", {}).get("cmcontinue")
        if not continue_param:
            break
        time.sleep(REQUEST_DELAY)

    return all_titles


def fetch_page_images(session: requests.Session, titles: List[str]) -> Dict[str, List[str]]:
    """
    Fetch the list of image filenames for a batch of pages.
    Returns a dict: { "Page Title": ["File:Image1.jpg", "File:Image2.png"] }
    """
    if not titles:
        return {}

    print(f"Fetching image info for {len(titles)} pages...")
    images_map: Dict[str, List[str]] = {}

    # Process in batches to avoid URLs that are too long
    batch_size = 50
    for i in range(0, len(titles), batch_size):
        batch = titles[i:i + batch_size]

        params = {
            "action": "query",
            "format": "json",
            "prop": "images",
            "titles": "|".join(batch),
            "imlimit": IMAGE_LIMIT,
        }

        try:
            response = session.get(WIKIPEDIA_API_URL, params=params, headers=HEADERS)
            response.raise_for_status()
            data = response.json()
        except Exception as e:
            print(f"Error fetching images: {e}")
            continue

        pages_data = data.get("query", {}).get("pages", {})
        for page_id, page_info in pages_data.items():
            # Skip if the page is missing or has no images
            if "missing" in page_info or "images" not in page_info:
                continue

            title = page_info.get("title")
            if title:
                image_files = [img["title"] for img in page_info["images"]]
                images_map[title] = image_files

        time.sleep(REQUEST_DELAY)

    return images_map


def main():
    print(f"Fetching pages from category: '{CATEGORY_TITLE}'...")

    with requests.Session() as session:
        page_titles = fetch_category_members(session, CATEGORY_TITLE, CM_LIMIT)

    if not page_titles:
        print("No pages found.")
        return

    print(f"\nTotal pages found: {len(page_titles)}")

    # Fetch images for all collected pages
    images_map = fetch_page_images(session, page_titles)

    # Display results
    print("\n--- Results (Page Title -> Image Files) ---")
    for title in page_titles:
        images = images_map.get(title, [])
        if images:
            print(f"\nPage: {title}")
            for img in images:
                print(f"  - {img}")
        else:
            print(f"\nPage: {title} (No images found on this page)")


if __name__ == "__main__":
    main()