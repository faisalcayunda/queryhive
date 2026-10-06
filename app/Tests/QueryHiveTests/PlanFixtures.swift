import Foundation

/// Plans captured from the dev servers on 2026-10-06 (PostgreSQL 17.11, Trino 483), verbatim.
/// Raw strings rather than test resources, so `Package.swift` does not change.
enum PlanFixtures {
    static let pgJoinPlain = #"""
[
  {
    "Plan": {
      "Node Type": "Nested Loop",
      "Parallel Aware": false,
      "Async Capable": false,
      "Join Type": "Inner",
      "Startup Cost": 1.37,
      "Total Cost": 114.65,
      "Plan Rows": 512,
      "Plan Width": 192,
      "Inner Unique": false,
      "Plans": [
        {
          "Node Type": "Hash Join",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Join Type": "Inner",
          "Startup Cost": 1.09,
          "Total Cost": 20.65,
          "Plan Rows": 68,
          "Plan Width": 132,
          "Inner Unique": true,
          "Hash Cond": "(c.relnamespace = n.oid)",
          "Plans": [
            {
              "Node Type": "Seq Scan",
              "Parent Relationship": "Outer",
              "Parallel Aware": false,
              "Async Capable": false,
              "Relation Name": "pg_class",
              "Alias": "c",
              "Startup Cost": 0.00,
              "Total Cost": 19.19,
              "Plan Rows": 68,
              "Plan Width": 72,
              "Filter": "(relkind = 'r'::\"char\")"
            },
            {
              "Node Type": "Hash",
              "Parent Relationship": "Inner",
              "Parallel Aware": false,
              "Async Capable": false,
              "Startup Cost": 1.04,
              "Total Cost": 1.04,
              "Plan Rows": 4,
              "Plan Width": 68,
              "Plans": [
                {
                  "Node Type": "Seq Scan",
                  "Parent Relationship": "Outer",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Relation Name": "pg_namespace",
                  "Alias": "n",
                  "Startup Cost": 0.00,
                  "Total Cost": 1.04,
                  "Plan Rows": 4,
                  "Plan Width": 68
                }
              ]
            }
          ]
        },
        {
          "Node Type": "Index Only Scan",
          "Parent Relationship": "Inner",
          "Parallel Aware": false,
          "Async Capable": false,
          "Scan Direction": "Forward",
          "Index Name": "pg_attribute_relid_attnam_index",
          "Relation Name": "pg_attribute",
          "Alias": "a",
          "Startup Cost": 0.28,
          "Total Cost": 1.30,
          "Plan Rows": 8,
          "Plan Width": 68,
          "Index Cond": "(attrelid = c.oid)"
        }
      ]
    }
  }
]
"""#

    static let pgAnalyzeLimitSubPlan = #"""
[
  {
    "Plan": {
      "Node Type": "Limit",
      "Parallel Aware": false,
      "Async Capable": false,
      "Startup Cost": 0.27,
      "Total Cost": 28.63,
      "Plan Rows": 5,
      "Plan Width": 72,
      "Actual Startup Time": 0.045,
      "Actual Total Time": 0.056,
      "Actual Rows": 5,
      "Actual Loops": 1,
      "Shared Hit Blocks": 32,
      "Shared Read Blocks": 2,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0,
      "Plans": [
        {
          "Node Type": "Index Scan",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Scan Direction": "Forward",
          "Index Name": "pg_class_relname_nsp_index",
          "Relation Name": "pg_class",
          "Alias": "c",
          "Startup Cost": 0.27,
          "Total Cost": 385.87,
          "Plan Rows": 68,
          "Plan Width": 72,
          "Actual Startup Time": 0.045,
          "Actual Total Time": 0.055,
          "Actual Rows": 5,
          "Actual Loops": 1,
          "Filter": "(relkind = 'r'::\"char\")",
          "Rows Removed by Filter": 44,
          "Shared Hit Blocks": 32,
          "Shared Read Blocks": 2,
          "Shared Dirtied Blocks": 0,
          "Shared Written Blocks": 0,
          "Local Hit Blocks": 0,
          "Local Read Blocks": 0,
          "Local Dirtied Blocks": 0,
          "Local Written Blocks": 0,
          "Temp Read Blocks": 0,
          "Temp Written Blocks": 0,
          "Plans": [
            {
              "Node Type": "Aggregate",
              "Strategy": "Plain",
              "Partial Mode": "Simple",
              "Parent Relationship": "SubPlan",
              "Subplan Name": "SubPlan 1",
              "Parallel Aware": false,
              "Async Capable": false,
              "Startup Cost": 4.44,
              "Total Cost": 4.45,
              "Plan Rows": 1,
              "Plan Width": 8,
              "Actual Startup Time": 0.006,
              "Actual Total Time": 0.006,
              "Actual Rows": 1,
              "Actual Loops": 5,
              "Shared Hit Blocks": 11,
              "Shared Read Blocks": 0,
              "Shared Dirtied Blocks": 0,
              "Shared Written Blocks": 0,
              "Local Hit Blocks": 0,
              "Local Read Blocks": 0,
              "Local Dirtied Blocks": 0,
              "Local Written Blocks": 0,
              "Temp Read Blocks": 0,
              "Temp Written Blocks": 0,
              "Plans": [
                {
                  "Node Type": "Index Only Scan",
                  "Parent Relationship": "Outer",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Scan Direction": "Forward",
                  "Index Name": "pg_attribute_relid_attnum_index",
                  "Relation Name": "pg_attribute",
                  "Alias": "a",
                  "Startup Cost": 0.28,
                  "Total Cost": 4.42,
                  "Plan Rows": 8,
                  "Plan Width": 0,
                  "Actual Startup Time": 0.004,
                  "Actual Total Time": 0.004,
                  "Actual Rows": 15,
                  "Actual Loops": 5,
                  "Index Cond": "(attrelid = c.oid)",
                  "Rows Removed by Index Recheck": 0,
                  "Heap Fetches": 0,
                  "Shared Hit Blocks": 11,
                  "Shared Read Blocks": 0,
                  "Shared Dirtied Blocks": 0,
                  "Shared Written Blocks": 0,
                  "Local Hit Blocks": 0,
                  "Local Read Blocks": 0,
                  "Local Dirtied Blocks": 0,
                  "Local Written Blocks": 0,
                  "Temp Read Blocks": 0,
                  "Temp Written Blocks": 0
                }
              ]
            }
          ]
        }
      ]
    },
    "Planning": {
      "Shared Hit Blocks": 96,
      "Shared Read Blocks": 3,
      "Shared Dirtied Blocks": 0,
      "Shared Written Blocks": 0,
      "Local Hit Blocks": 0,
      "Local Read Blocks": 0,
      "Local Dirtied Blocks": 0,
      "Local Written Blocks": 0,
      "Temp Read Blocks": 0,
      "Temp Written Blocks": 0
    },
    "Planning Time": 0.754,
    "Triggers": [
    ],
    "Execution Time": 0.131
  }
]
"""#

    static let pgAnalyzeJoin = #"""
[
  {
    "Plan": {
      "Node Type": "Hash Join",
      "Parallel Aware": false,
      "Async Capable": false,
      "Join Type": "Inner",
      "Startup Cost": 21.82,
      "Total Cost": 116.38,
      "Plan Rows": 1589,
      "Plan Width": 128,
      "Actual Startup Time": 0.084,
      "Actual Total Time": 0.522,
      "Actual Rows": 2481,
      "Actual Loops": 1,
      "Inner Unique": true,
      "Hash Cond": "(a.attrelid = c.oid)",
      "Plans": [
        {
          "Node Type": "Seq Scan",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Relation Name": "pg_attribute",
          "Alias": "a",
          "Startup Cost": 0.00,
          "Total Cost": 86.26,
          "Plan Rows": 3126,
          "Plan Width": 68,
          "Actual Startup Time": 0.007,
          "Actual Total Time": 0.199,
          "Actual Rows": 3126,
          "Actual Loops": 1
        },
        {
          "Node Type": "Hash",
          "Parent Relationship": "Inner",
          "Parallel Aware": false,
          "Async Capable": false,
          "Startup Cost": 19.19,
          "Total Cost": 19.19,
          "Plan Rows": 211,
          "Plan Width": 68,
          "Actual Startup Time": 0.071,
          "Actual Total Time": 0.071,
          "Actual Rows": 211,
          "Actual Loops": 1,
          "Hash Buckets": 1024,
          "Original Hash Buckets": 1024,
          "Hash Batches": 1,
          "Original Hash Batches": 1,
          "Peak Memory Usage": 29,
          "Plans": [
            {
              "Node Type": "Seq Scan",
              "Parent Relationship": "Outer",
              "Parallel Aware": false,
              "Async Capable": false,
              "Relation Name": "pg_class",
              "Alias": "c",
              "Startup Cost": 0.00,
              "Total Cost": 19.19,
              "Plan Rows": 211,
              "Plan Width": 68,
              "Actual Startup Time": 0.004,
              "Actual Total Time": 0.049,
              "Actual Rows": 211,
              "Actual Loops": 1,
              "Filter": "(relkind = ANY ('{r,v}'::\"char\"[]))",
              "Rows Removed by Filter": 204
            }
          ]
        }
      ]
    },
    "Planning Time": 1.310,
    "Triggers": [
    ],
    "Execution Time": 0.641
  }
]
"""#

    static let trinoDistributed = #"""
{
  "0" : {
    "id" : "15",
    "name" : "Output",
    "descriptor" : {
      "columnNames" : "[name, _col1]"
    },
    "outputs" : [ {
      "type" : "varchar(25)",
      "name" : "name"
    }, {
      "type" : "bigint",
      "name" : "count"
    } ],
    "details" : [ "_col1 := count" ],
    "estimates" : [ {
      "outputRowCount" : 1500.0,
      "outputSizeInBytes" : 48000.0,
      "cpuCost" : 0.0,
      "memoryCost" : 0.0,
      "networkCost" : 0.0
    } ],
    "children" : [ {
      "id" : "10",
      "name" : "Aggregate",
      "descriptor" : {
        "type" : "FINAL",
        "keys" : "[name]"
      },
      "outputs" : [ {
        "type" : "varchar(25)",
        "name" : "name"
      }, {
        "type" : "bigint",
        "name" : "count"
      } ],
      "details" : [ "count := count(count_3)" ],
      "estimates" : [ {
        "outputRowCount" : 1500.0,
        "outputSizeInBytes" : 48000.0,
        "cpuCost" : 480000.0,
        "memoryCost" : 48000.0,
        "networkCost" : 0.0
      } ],
      "children" : [ {
        "id" : "300",
        "name" : "LocalExchange",
        "descriptor" : {
          "partitioning" : "HASH",
          "isReplicateNullsAndAny" : "",
          "arguments" : "[name::varchar(25)]"
        },
        "outputs" : [ {
          "type" : "varchar(25)",
          "name" : "name"
        }, {
          "type" : "bigint",
          "name" : "count_3"
        } ],
        "details" : [ ],
        "estimates" : [ {
          "outputRowCount" : 15000.0,
          "outputSizeInBytes" : 480000.0,
          "cpuCost" : 480000.0,
          "memoryCost" : 0.0,
          "networkCost" : 0.0
        } ],
        "children" : [ {
          "id" : "306",
          "name" : "RemoteSource",
          "descriptor" : {
            "sourceFragmentIds" : "[1]"
          },
          "outputs" : [ {
            "type" : "varchar(25)",
            "name" : "name"
          }, {
            "type" : "bigint",
            "name" : "count_3"
          } ],
          "details" : [ ],
          "estimates" : [ ],
          "children" : [ ]
        } ]
      } ]
    } ]
  },
  "1" : {
    "id" : "311",
    "name" : "Aggregate",
    "descriptor" : {
      "type" : "INTERMEDIATE",
      "keys" : "[name]"
    },
    "outputs" : [ {
      "type" : "varchar(25)",
      "name" : "name"
    }, {
      "type" : "bigint",
      "name" : "count_3"
    } ],
    "details" : [ "count_3 := count(count_3)" ],
    "estimates" : [ {
      "outputRowCount" : 15000.0,
      "outputSizeInBytes" : 480000.0,
      "cpuCost" : "NaN",
      "memoryCost" : "NaN",
      "networkCost" : "NaN"
    } ],
    "children" : [ {
      "id" : "180",
      "name" : "InnerJoin",
      "descriptor" : {
        "criteria" : "(custkey_1 = custkey)",
        "distribution" : "REPLICATED"
      },
      "outputs" : [ {
        "type" : "bigint",
        "name" : "count_3"
      }, {
        "type" : "varchar(25)",
        "name" : "name"
      } ],
      "details" : [ "Distribution: REPLICATED", "dynamicFilterAssignments = {custkey -> #df_236}" ],
      "estimates" : [ {
        "outputRowCount" : 15000.0,
        "outputSizeInBytes" : 480000.0,
        "cpuCost" : 798000.0,
        "memoryCost" : 48000.0,
        "networkCost" : 0.0
      } ],
      "children" : [ {
        "id" : "304",
        "name" : "Aggregate",
        "descriptor" : {
          "type" : "PARTIAL",
          "keys" : "[custkey_1]"
        },
        "outputs" : [ {
          "type" : "bigint",
          "name" : "custkey_1"
        }, {
          "type" : "bigint",
          "name" : "count_3"
        } ],
        "details" : [ "count_3 := count(*)" ],
        "estimates" : [ {
          "outputRowCount" : 15000.0,
          "outputSizeInBytes" : 270000.0,
          "cpuCost" : "NaN",
          "memoryCost" : "NaN",
          "networkCost" : "NaN"
        } ],
        "children" : [ {
          "id" : "237",
          "name" : "ScanFilter",
          "descriptor" : {
            "table" : "tpch:tiny:orders",
            "filterPredicate" : "",
            "dynamicFilters" : "{custkey_1 = #df_236}"
          },
          "outputs" : [ {
            "type" : "bigint",
            "name" : "custkey_1"
          } ],
          "details" : [ "custkey_1 := tpch:custkey", "tpch:orderstatus", "    :: [[F], [O], [P]]" ],
          "estimates" : [ {
            "outputRowCount" : 15000.0,
            "outputSizeInBytes" : 135000.0,
            "cpuCost" : 135000.0,
            "memoryCost" : 0.0,
            "networkCost" : 0.0
          }, {
            "outputRowCount" : 15000.0,
            "outputSizeInBytes" : 135000.0,
            "cpuCost" : 135000.0,
            "memoryCost" : 0.0,
            "networkCost" : 0.0
          } ],
          "children" : [ ]
        } ]
      }, {
        "id" : "272",
        "name" : "LocalExchange",
        "descriptor" : {
          "partitioning" : "SINGLE",
          "isReplicateNullsAndAny" : "",
          "arguments" : "[]"
        },
        "outputs" : [ {
          "type" : "bigint",
          "name" : "custkey"
        }, {
          "type" : "varchar(25)",
          "name" : "name"
        } ],
        "details" : [ ],
        "estimates" : [ {
          "outputRowCount" : 1500.0,
          "outputSizeInBytes" : 48000.0,
          "cpuCost" : 0.0,
          "memoryCost" : 0.0,
          "networkCost" : 0.0
        } ],
        "children" : [ {
          "id" : "210",
          "name" : "RemoteSource",
          "descriptor" : {
            "sourceFragmentIds" : "[2]"
          },
          "outputs" : [ {
            "type" : "bigint",
            "name" : "custkey"
          }, {
            "type" : "varchar(25)",
            "name" : "name"
          } ],
          "details" : [ ],
          "estimates" : [ ],
          "children" : [ ]
        } ]
      } ]
    } ]
  },
  "2" : {
    "id" : "0",
    "name" : "TableScan",
    "descriptor" : {
      "table" : "tpch:tiny:customer"
    },
    "outputs" : [ {
      "type" : "bigint",
      "name" : "custkey"
    }, {
      "type" : "varchar(25)",
      "name" : "name"
    } ],
    "details" : [ "custkey := tpch:custkey", "name := tpch:name" ],
    "estimates" : [ {
      "outputRowCount" : 1500.0,
      "outputSizeInBytes" : 48000.0,
      "cpuCost" : 48000.0,
      "memoryCost" : 0.0,
      "networkCost" : 0.0
    } ],
    "children" : [ ]
  }
}
"""#

    static let trinoLogical = #"""
{
  "0" : {
    "id" : "5",
    "name" : "Output",
    "descriptor" : {
      "columnNames" : "[nationkey, name, regionkey, comment]"
    },
    "outputs" : [ {
      "type" : "bigint",
      "name" : "nationkey"
    }, {
      "type" : "varchar(25)",
      "name" : "name"
    }, {
      "type" : "bigint",
      "name" : "regionkey"
    }, {
      "type" : "varchar(152)",
      "name" : "comment"
    } ],
    "details" : [ ],
    "estimates" : [ {
      "outputRowCount" : 25.0,
      "outputSizeInBytes" : 2734.0,
      "cpuCost" : 0.0,
      "memoryCost" : 0.0,
      "networkCost" : 0.0
    } ],
    "children" : [ {
      "id" : "0",
      "name" : "TableScan",
      "descriptor" : {
        "table" : "tpch:tiny:nation"
      },
      "outputs" : [ {
        "type" : "bigint",
        "name" : "nationkey"
      }, {
        "type" : "varchar(25)",
        "name" : "name"
      }, {
        "type" : "bigint",
        "name" : "regionkey"
      }, {
        "type" : "varchar(152)",
        "name" : "comment"
      } ],
      "details" : [ "comment := tpch:comment", "regionkey := tpch:regionkey", "nationkey := tpch:nationkey", "name := tpch:name" ],
      "estimates" : [ {
        "outputRowCount" : 25.0,
        "outputSizeInBytes" : 2734.0,
        "cpuCost" : 2734.0,
        "memoryCost" : 0.0,
        "networkCost" : 0.0
      } ],
      "children" : [ ]
    } ]
  }
}
"""#

}
