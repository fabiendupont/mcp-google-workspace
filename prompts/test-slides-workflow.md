Create a Google Slides presentation named "Quarterly Review" in Drive folder 1VkrXx3bhkQaLJEmnY_t5VxVmBETfsbY1.

Use gws_slides_import_marp to create the initial presentation with this Marp Markdown:

```
---
marp: true
---

<!-- _class: title -->

# Quarterly Review
Q2 2026 Results

---

# Revenue Summary

- Total revenue: $4.2M (up 15% YoY)
- Enterprise segment: $2.8M
- SMB segment: $1.4M

<!-- notes -->
Revenue exceeded forecast by 8%

---

# Key Wins

- Signed 12 new enterprise accounts
- Launched self-service onboarding
- Reduced churn to 3.2%

---

# Challenges

- Hiring pipeline slower than planned
- Infrastructure costs rose 22%
- Two major outages in April

---

# Next Quarter Goals

- Hire 8 engineers by end of Q3
- Launch EU data center
- Ship v2.0 of the analytics platform
```

After creating the presentation:

1. Read back the full presentation using gws_slides_read to verify all 5 slides exist with their titles and content

2. Read slide 2 specifically to verify the Revenue Summary content

3. Read the presentation in markdown format to get a Marp-like view

4. Add a new slide at position 4 (between "Key Wins" and "Challenges") with this content:
   # Customer Satisfaction
   - NPS score: 72 (up from 65)
   - Support ticket resolution: 4.2 hours average
   - Customer retention rate: 96.8%

5. Read back the presentation again to verify it now has 6 slides and the new slide is at position 4

6. Delete slide 5 (which should now be "Challenges")

7. Reorder slides so slide 5 ("Next Quarter Goals") moves to position 2

8. Duplicate slide 3 (Revenue Summary) using gws_slides_duplicate

9. Update the duplicated slide's title to "Revenue Summary (Backup)" using gws_slides_update

10. Read the final presentation to verify:
   - 6 slides total
   - Slide 1: Quarterly Review (title)
   - Slide 2: Next Quarter Goals
   - Slide 3: Revenue Summary
   - Slide 4: Revenue Summary (Backup)
   - Slide 5: Key Wins
   - Slide 6: Customer Satisfaction
