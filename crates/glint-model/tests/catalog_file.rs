//! A translator's catalog file read at start (`GLINT_LANGUAGE=D:\work\zh-CN.po`). In a test binary of its own:
//! the file stands in for the built-in catalog for the whole process, which the unit tests mustn't see.

use glint_model::i18n::{self, Language};

#[test]
fn a_catalog_file_stands_in_for_the_built_in_one() {
    let folder = std::env::temp_dir().join(format!("glint-catalog-file-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();

    let unnamed = folder.join("chinese.po");
    std::fs::write(&unnamed, "").unwrap();
    let error = i18n::use_catalog_file(&unnamed).unwrap_err();
    assert!(
        error.ends_with("the file's name says no language Glint speaks"),
        "{error}"
    );

    let file = folder.join("zh-CN.po");
    std::fs::write(
        &file,
        "msgid \"\"\nmsgstr \"Language: zh_CN\\n\"\n\n\
         msgid \"A text only this file has\"\nmsgstr \"只有这个文件有的文字\"\n\n\
         msgid \"Delete {name}?\"\nmsgstr \"删除？\"\n\n\
         msgid \"Not translated yet\"\nmsgstr \"\"\n",
    )
    .unwrap();
    let read = i18n::use_catalog_file(&file).unwrap();
    assert_eq!(read.language, Language::ZhHans);
    assert_eq!(read.translated, 1);
    assert_eq!(
        read.left_out,
        ["line 7: \"Delete {name}?\": the translation lacks {name}"]
    );

    assert_eq!(
        i18n::translate(Language::ZhHans, "A text only this file has"),
        "只有这个文件有的文字"
    );
    assert_eq!(i18n::translate(Language::ZhHans, "Delete {name}?"), "Delete {name}?");
    assert_eq!(
        i18n::translate(Language::ZhHans, "Not translated yet"),
        "Not translated yet"
    );
    assert_eq!(i18n::translated_count(Language::ZhHans), 1);
    assert_eq!(
        i18n::translate(Language::En, "A text only this file has"),
        "A text only this file has"
    );
    assert!(i18n::use_catalog_file(&file).unwrap_err().contains("already read"));

    std::fs::remove_dir_all(&folder).ok();
}
