mod support;

use ysda::schema::SqliteCatalog;

#[test]
fn inspects_user_tables_columns_and_primary_keys() {
    let database = support::create_test_database();

    let snapshot =
        SqliteCatalog::inspect(&database.path).expect("schema inspection should succeed");

    assert_eq!(snapshot.tables.len(), 2);
    assert_eq!(snapshot.tables[0].name, "customers");
    assert_eq!(snapshot.tables[0].columns[0].name, "id");
    assert_eq!(snapshot.tables[0].columns[0].primary_key_position, 1);
    assert!(snapshot.tables[0].columns[1].not_null);
    assert_eq!(snapshot.tables[1].name, "orders");
}
