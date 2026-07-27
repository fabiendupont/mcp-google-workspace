Work with a Google Slides presentation using a branded template. All files go in Drive folder 1VkrXx3bhkQaLJEmnY_t5VxVmBETfsbY1.

1. Call gws_templates to list available templates. Examine the placeholder details for each layout — note which placeholders have large/medium/small sizes and their positions.

2. Create a presentation named "Template Test" from the template using gws_slides_import_marp:
   ```
   ---
   marp: true
   ---

   # Template Test
   Branded presentation demo
   ```

   Use the template name from step 1.

3. Read the presentation with gws_slides_read to confirm it was created with the template's styling.

4. Add a new slide at position 2 using gws_slides_add with an "Interior title and body" layout. Use this marp:
   ```
   # Key Findings

   - Finding one: performance improved by 40%
   - Finding two: cost reduced by $2.1M annually
   - Finding three: deployment time cut from 3 days to 4 hours
   ```

5. Read slide 2 to verify the content was placed correctly.

6. Add another slide at position 3 using gws_slides_add with a blank marp content and a background image. Use this public image URL as background_image: https://www.google.com/images/branding/googlelogo/2x/googlelogo_color_272x92dp.png

   Use marp content: (empty or just a space character)

7. Now use gws_slides_update with the placeholders parameter to update slide 2. Set these placeholders:
   - TITLE: "Updated Key Findings"
   - If the slide has a SUBTITLE[1] placeholder, set it to "Q3 2026 Analysis"

8. Read back slide 2 to verify the title was updated to "Updated Key Findings".

9. Read the full presentation to verify:
   - At least 3 slides exist
   - Slide 1 has "Template Test" as title
   - Slide 2 has "Updated Key Findings" as title
