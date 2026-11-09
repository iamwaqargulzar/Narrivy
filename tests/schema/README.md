# Schema / Import Safety Tests

P1/P3/P5/P6 implement tests for:
- valid Mode/Profile/Privacy/Model examples;
- unknown required versions;
- oversized strings/arrays;
- duplicate IDs;
- invalid closed permission names;
- missing model provenance/license/hash fields;
- absolute/`..` archive paths and unsafe symlink/hardlink extraction attempts;
- checksum mismatch/partial archive;
- regex/path inputs that exceed implementation bounds;
- atomic import: one bad entry cannot leave a partially active pack/model/profile set.
