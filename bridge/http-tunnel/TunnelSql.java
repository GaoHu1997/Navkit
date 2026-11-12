// Appended to the Java 11 service compilation unit. Every token is consumed.
final class TunnelSql {
  static final class Token {final String text;final boolean quoted,literal;Token(String t,boolean q,boolean l){text=t;quoted=q;literal=l;}boolean keyword(String w){return !quoted&&!literal&&text.equalsIgnoreCase(w);}}
  static final class TableRef {final String schema,name;TableRef(String s,String n){schema=s;name=n;}}
  final List<Token> tokens;final String namespace;int pos,parameters;boolean write;final List<TableRef> tables=new ArrayList<>();
  TunnelSql(String sql,String namespace)throws SQLException{this.namespace=namespace;tokens=lex(sql);}
  static TunnelSql parse(String sql,String namespace)throws SQLException{TunnelSql ast=new TunnelSql(sql,namespace);ast.statement();if(ast.pos!=ast.tokens.size())throw invalid();return ast;}
  static SQLException invalid(){return JdbcMutationSupport.fault("UNSUPPORTED_SQL","0A000","Use one complete supported SELECT or parameterized INSERT/UPDATE/DELETE; functions, DDL, scripts and transaction statements are not supported");}
  static List<Token> lex(String sql)throws SQLException {
    if(sql.isEmpty()||sql.length()>1024*1024||sql.indexOf('\0')>=0)throw invalid();List<Token> result=new ArrayList<>();
    for(int i=0;i<sql.length();){char c=sql.charAt(i);if(c==' '||c=='\t'||c=='\r'||c=='\n'){i++;continue;}
      if(c=='\''||c=='"'){boolean literal=c=='\'';char quote=c;StringBuilder text=new StringBuilder();boolean ended=false;i++;while(i<sql.length()){char next=sql.charAt(i++);if(next==quote){if(i<sql.length()&&sql.charAt(i)==quote){text.append(quote);i++;}else{ended=true;break;}}else text.append(next);}if(!ended||text.length()>1024*1024)throw invalid();result.add(new Token(text.toString(),!literal,literal));}
      else if(c>='a'&&c<='z'||c>='A'&&c<='Z'||c=='_'){int start=i++;while(i<sql.length()){char next=sql.charAt(i);if(Character.isLetterOrDigit(next)||next=='_'||next=='$')i++;else break;}result.add(new Token(sql.substring(start,i),false,false));}
      else if(c>='0'&&c<='9'){int start=i++;while(i<sql.length()&&sql.charAt(i)>='0'&&sql.charAt(i)<='9')i++;if(i<sql.length()&&sql.charAt(i)=='.'){i++;int before=i;while(i<sql.length()&&sql.charAt(i)>='0'&&sql.charAt(i)<='9')i++;if(before==i)throw invalid();}result.add(new Token(sql.substring(start,i),false,true));}
      else if("(),.*?=<>!+-".indexOf(c)>=0){String text=Character.toString(c);i++;if(i<sql.length()&&(c=='<'||c=='>'||c=='!')&&sql.charAt(i)=='='){text+='=';i++;}else if(c=='<'&&i<sql.length()&&sql.charAt(i)=='>'){text+='>';i++;}result.add(new Token(text,false,false));}
      else throw invalid();if(result.size()>16384)throw invalid();
    }return result;
  }
  Token peek(){return pos<tokens.size()?tokens.get(pos):null;}
  boolean take(String text){Token t=peek();if(t!=null&&t.keyword(text)){pos++;return true;}return false;}
  void need(String text)throws SQLException{if(!take(text))throw invalid();}
  boolean is(String text){return peek()!=null&&peek().keyword(text);}
  String ident()throws SQLException{Token t=peek();if(t==null||t.literal||!t.quoted&&!t.text.matches("[A-Za-z_][A-Za-z0-9_$]*"))throw invalid();pos++;String name=t.quoted?t.text:t.text.toUpperCase(Locale.ROOT);if(name.isEmpty()||name.length()>200)throw invalid();return name;}
  void column()throws SQLException{ident();if(take(".")){ident();if(take("."))throw invalid();}}
  void value()throws SQLException{if(take("?")){parameters++;return;}if(take("NULL")||take("TRUE")||take("FALSE"))return;boolean negative=take("-");Token t=peek();if(t==null||!t.literal||negative&&!t.text.matches("[0-9]+(?:\\.[0-9]+)?"))throw invalid();pos++;}
  void projection()throws SQLException{if(take("*"))return;if(is("COUNT")||is("SUM")||is("MIN")||is("MAX")||is("AVG")){String function=ident();need("(");if(function.equals("COUNT")&&take("*")){}else column();need(")");}else if(peek()!=null&&peek().literal)value();else column();if(take("AS"))ident();}
  void table(boolean alias)throws SQLException{String schema=namespace,name=ident();if(take(".")){schema=name;name=ident();}if(!schema.equals(namespace))throw JdbcMutationSupport.fault("NAMESPACE_FORBIDDEN","42501","Table is outside the session namespace");tables.add(new TableRef(schema,name));if(tables.size()>8)throw invalid();if(alias&&take("AS"))ident();}
  void predicate(int depth)throws SQLException{if(depth>24)throw invalid();if(take("(")){expression(depth+1);need(")");return;}if(take("NOT")){predicate(depth+1);return;}column();if(take("IS")){take("NOT");need("NULL");return;}if(take("IN")){need("(");value();int count=1;while(take(",")){value();if(++count>1024)throw invalid();}need(")");return;}if(take("BETWEEN")){value();need("AND");value();return;}if(take("LIKE")){value();return;}if(take("=")||take("<>")||take("!=")||take("<=")||take(">=")||take("<")||take(">")){if(peek()!=null&&!peek().literal&&!is("NULL")&&!is("TRUE")&&!is("FALSE")&&!is("?")&&!is("-"))column();else value();return;}throw invalid();}
  void expression(int depth)throws SQLException{predicate(depth);while(take("AND")||take("OR"))predicate(depth);}
  void statement()throws SQLException{
    if(take("SELECT")){projection();while(take(","))projection();if(take("FROM")){table(true);while(take(","))table(true);}if(take("WHERE")){if(tables.isEmpty())throw invalid();expression(0);}if(take("ORDER")){need("BY");order();while(take(","))order();}if(take("LIMIT")){value();if(take("OFFSET"))value();}}
    else if(take("INSERT")){write=true;need("INTO");table(false);need("(");ident();while(take(","))ident();need(")");need("VALUES");need("(");value();while(take(","))value();need(")");}
    else if(take("UPDATE")){write=true;table(false);need("SET");assignment();while(take(","))assignment();need("WHERE");expression(0);}
    else if(take("DELETE")){write=true;need("FROM");table(false);need("WHERE");expression(0);}else throw invalid();
  }
  void assignment()throws SQLException{ident();need("=");value();}
  void order()throws SQLException{column();if(!take("ASC"))take("DESC");if(take("NULLS")){if(!take("FIRST"))need("LAST");}}
}
